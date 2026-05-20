# Sprint 41 — F14 v2: FBM noise + river-carve brush

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 41** — implements the remaining F14 procgen pieces.
Sprint 9 / ADR-020 shipped the **math-function subset** (evalexpr-
based per-pixel expressions). Sprint 37 shipped the **hydraulic
erosion brush**. Sprint 41 closes F14 with:

- **FBM (Fractional Brownian Motion) noise** as a procgen primitive
  — multi-octave Perlin/Simplex composition. Adds presets like
  "Hilly terrain", "Archipelago", "Canyon system".
- **River-carve brush** — interactive: draw a line on the canvas
  → carve a river bed with banks. Optional water-mode integration
  (terrain below sea level along the line).

**Prerequisites:**
- Sprint 40 (F13 import) — F-list close-out continues.
- Sprint 37 (brushes + erosion) — F2 sculpt brushes are extensible.
- Sprint 24 (multithreading) — FBM noise generation parallelises
  trivially.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F14 (math subset shipped;
   FBM, erosion, river carve listed as remaining).
3. `/home/teague/code/BARMapEditor/crates/barme-core/src/procgen.rs`
   — the math-function path. FBM lives here.
4. `/home/teague/code/BARMapEditor/crates/barme-core/src/brushes/`
   — extend with `river_carve.rs`.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-41-fbm-noise
./devlog/log.sh new sprint-41-river-carve-brush
```

## Step 3 — Scope

### 1. FBM noise function

`crates/barme-core/src/procgen.rs` extension:

```rust
pub fn fbm(x: f32, z: f32, octaves: u32, lacunarity: f32, persistence: f32, seed: u32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut max_value = 0.0;
    for _ in 0..octaves {
        total += amplitude * perlin(x * frequency, z * frequency, seed);
        max_value += amplitude;
        amplitude *= persistence;
        frequency *= lacunarity;
    }
    total / max_value  // normalise to [-1, 1]
}
```

Expose as a builtin in the procgen evalexpr context:
`fbm(x, z, octaves=4, lacunarity=2.0, persistence=0.5)`.

### 2. New procgen presets

`crates/barme-core/src/procgen.rs::presets`:
- **Hilly terrain**: `fbm(x, z, 5, 2.0, 0.5) * 256 + 0.5`
- **Archipelago**: `fbm(x, z, 4, 2.0, 0.5) * 256` with sea-level
  threshold.
- **Canyon system**: combination of FBM + flat-bottom carve.

Each preset lands as a `ProcgenPreset` enum variant + procgen
inspector chip + help-text entry.

### 3. River-carve brush

`crates/barme-core/src/brushes/river_carve.rs` (new):

```rust
pub struct RiverCarveBrush {
    points: Vec<(f32, f32)>,  // user-drawn polyline
    depth: f32,                // elmos below current terrain
    width: f32,                // elmos
    bank_height: f32,          // optional banks
}
```

UX: in Sculpt mode with RiverCarve selected:
1. LMB-click adds a point to the polyline.
2. LMB-drag adds a point per N pixels (continuous).
3. Right-click or Enter commits — terrain is carved along the
   polyline, with quadratic falloff on bank slopes.
4. Esc cancels in progress.

Commit emits a single `BrushStamp` with the full polyline path;
undo restores the pre-carve heightmap.

### 4. Water-mode integration

If `Project.water_mode != None` and the river-carve depth puts
terrain below `min_height = 0`, the carved channel renders as
water in the editor preview (Sprint 26's water plane samples
correctly).

### 5. Inspector wiring

`inspector_sculpt` gains a RiverCarve brush card (`t.info` cyan
colour). Polyline-in-progress state stored in
`App::river_carve_state`.

### 6. Tests + rollup

- **FBM determinism**: same seed → same output.
- **River-carve undo**: carve → Ctrl+Z → terrain restored.
- **Water-mode regression**: carving below sea-level → water
  renders correctly.
- **Rollup**: STATUS UPDATEs (F14 fully closed; Stage 2
  progressing).

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on FBM bench at 16-SMU;
`trace!` on polyline-point captures; `info!` on river-carve commit
with point count.

## Step 5 — Out of scope

- **Hydraulic erosion procedural preset** (vs the manual brush
  from Sprint 37) — defer; presets compose easily.
- **Voronoi / cellular noise** — out of scope.
- **Thermal erosion** — Stage 2 polish.

## Step 6 — Critical pitfalls

1. **FBM perf**: 5 octaves × 16M pixels = 80M Perlin calls. Use
   Sprint 24's rayon path. Bench: < 500 ms on 16-SMU on dev box.

2. **Perlin choice**: use a well-known implementation (e.g.,
   `noise` crate's `Perlin`). Don't roll your own — Perlin's
   gradient-table edge cases are subtle.

3. **River-carve polyline tail**: end the polyline with a "fade
   out" that returns to natural terrain to avoid an abrupt
   ditch end.

4. **Polyline + symmetry**: river-carve replicates across
   symmetry axes like any brush stroke.

5. **Banks vs flatness**: the bank-slope parameter controls
   how steep the river banks are. Default ~ 1:3 slope.

## Step 7 — Exit criteria

- 4+ commits on `main`: FBM function + presets, river-carve
  brush, inspector wiring, rollup.
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (F14 fully closed).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: select "Hilly terrain" preset → apply → terrain has
  FBM character. River-carve a curve → terrain shows channel.
- Final devlog: summary + "Sprint 42 = F15 type-map editor"
  handoff.
