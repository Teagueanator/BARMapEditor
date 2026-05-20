# Sprint 42 — F15 Type-map editor + per-terraintype gameplay params

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 42** — implements **F15** (type-map editor + per-
terraintype gameplay params) from the SRS Stage-2 list.

In BAR, the **type map** is an 8-bit-per-pixel mask where each
value (0–255) indexes into a `terrain_types` table in mapinfo.
Each terrain type has properties: `hardness`, `receive_tracks`,
per-locomotor `move_speeds` (e.g., "tank speed 0.8, bot speed
1.0, hover speed 1.2").

Today the schema seeds 4 entries (Default / Rock / Sand / Water)
but there's no UI editor and no canvas-side painter. Sprint 42
ships both:

1. **Tool::TypeMap** — new tool (next available accelerator).
   LMB paints the active terrain-type ID onto the type map.
2. **Type-types editor** — F9 form's Terrain Types tab gains row
   editing for hardness / receive_tracks / move_speeds.

**Prerequisites:**
- Sprint 41 (F14 v2) — F-list progress continues.
- Sprint 34 (grass) — grass density derives from terrain_type[0]
  mask; this sprint makes that mask user-controllable.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F15.
3. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — `TerrainType` struct.
4. `/home/teague/code/BARMapEditor/crates/barme-core/src/project.rs`
   — add `Project.type_map: TypeMap` (R8 storage).

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-42-typemap-editor
```

## Step 3 — Scope

### 1. TypeMap data model

`crates/barme-core/src/type_map.rs` (new):

```rust
pub struct TypeMap {
    pub data: Vec<u8>,
    pub dim: (u32, u32),  // 32*smu_x × 32*smu_z
}

impl TypeMap {
    pub fn paint_at(&mut self, center: Vec2, radius: f32, type_id: u8, falloff: bool);
    pub fn sample(&self, x: u32, z: u32) -> u8;
}
```

Persisted as a PNG sidecar (`<project>/typemap.png` — 8-bit
grayscale).

### 2. Tool::TypeMap

Add `Tool::TypeMap` variant (free keyboard accelerator —
preferably `T`).

Inspector:
- Active terrain type ID dropdown (lists `Project.mapinfo.terrain_types`
  with names).
- Brush radius / strength sliders.
- "Show overlay" toggle to color-tint the canvas by terrain type.

LMB drags paint; symmetry replicates.

### 3. Terrain types F9 form tab

Already shipped scaffold in Sprint 18 / C7. Sprint 42 fills in:
- Add row button — appends a new terrain type with defaults.
- Remove row button per row (with confirm modal).
- Per-row inline editors: name (TextEdit), hardness (DragValue),
  receive_tracks (checkbox), move_speeds (4-DragValue grid:
  tank / bot / hover / ship).

### 4. Pipeline emission

`crates/barme-pipeline/src/sd7.rs`:
- Pack the typemap PNG into the SMF chunk that the engine reads
  for terrain-type lookup.
- Emit each terrain type's properties into mapinfo.lua's
  `terrain_types` table.

### 5. Grass density wiring

Sprint 34 stubbed density from terrain_type[0]. Sprint 42 wires
the real path:
```rust
pub fn bake_grass_density(type_map: &TypeMap, grass: &GrassBlock) -> GrassDensity {
    let mut density = vec![0u8; type_map.data.len()];
    for (i, &type_id) in type_map.data.iter().enumerate() {
        density[i] = if type_id == 0 { 255 } else { 0 };
    }
    // slope falloff layer...
}
```

### 6. Lint integration

Add lint rule `TerrainTypeIdOutOfRange` — if the type map has a
pixel value > `terrain_types.len()`, surface a warning.

### 7. Tests + rollup

- **TypeMap paint**: paint a 16-radius circle with type 1 →
  pixels within radius are 1; outside are unchanged.
- **F9 form round-trip**: edit a terrain type's hardness → save
  → re-open → preserved.
- **Grass density wiring**: type-map painted with type 1 in a
  region → grass density there drops to 0.
- **Rollup**: STATUS UPDATEs (F15 done).

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on type-map size +
unique-type-count; `trace!` on paint commits.

## Step 5 — Out of scope

- **Per-terraintype texture variation** (different textures per
  type, beyond the splat distribution) — Stage 2 polish.
- **Visualisation as overlay color-tinted minimap** — Stage 2.

## Step 6 — Critical pitfalls

1. **TypeMap dim vs heightmap dim**: typemap is `32*smu_x` while
   heightmap is `64*smu_x + 1`. Resampling on resize.

2. **Default type 0 fills empty regions**: when adding a new
   project, init the entire type_map to 0.

3. **PNG palette indexed vs grayscale**: 8-bit grayscale PNG
   stores the type ID directly. Avoid palette format.

4. **F15 lint coupling**: Sprint 21's `TeamsLessThanSixteenOnLargeMap`
   etc. don't touch terrain_types; no regression.

5. **Engine reads typemap from SMF chunk**, NOT a separate Lua
   file. Verify the SMF reader / writer covers this.

## Step 7 — Exit criteria

- 4+ commits on `main`: data model + tool, F9 tab editing,
  pipeline emission, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (F15 done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: paint type 1 in a region → grass disappears there
  in editor preview → build → BAR shows tank-speed=0.8 in that
  region.
- Final devlog: summary + "Sprint 43 = F16 skybox library" handoff.
