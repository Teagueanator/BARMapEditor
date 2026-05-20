# Sprint 37 — Flatten / erode / ramp brushes + arbitrary-axis symmetry line picker

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 37** — the first Stage-2 feature sprint. Stage 1 +
renderer-parity arc are complete; this sprint adds the SRS-promised
sculpt brushes that didn't ship in Sprint 1 + the arbitrary-axis
symmetry line picker (deferred from Sprint 1 / B2).

**F2** (real-time heightmap sculpting) lists *raise / lower / flatten
/ smooth / erode / ramp*. Sprints 1+ shipped only raise / lower /
smooth. Sprint 37 ships **flatten / erode / ramp**.

**F3** (symmetry enforcement) lists *H / V / Quad / both diagonals
+ rotational fold ∈ 2..=12 + arbitrary-axis line picker*. The line
picker has been deferred since Sprint 3 / B2 — Sprint 37 finally
ships it.

After this sprint, F2 + F3 are fully closed per the SRS commitments.

**Prerequisites:**
- Stage 1 + renderer-parity arc (Sprints 1-36) complete.
- Sprint 24 (multithreading) — the new brushes will hit larger
  footprints; the per-row rayon pattern is established for the
  smooth brush; the new brushes adopt it.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F2 brush list, F3
   symmetry list.
3. `/home/teague/code/BARMapEditor/docs/DECISIONS.md` — ADR-018
   (Brush trait) + ADR-019 (symmetry replication). Sprint 37
   extends both.
4. `/home/teague/code/BARMapEditor/crates/barme-core/src/brushes/`
   — existing raise / lower / smooth. New brushes plug into the
   same trait.
5. `/home/teague/code/BARMapEditor/crates/barme-core/src/symmetry.rs`
   — extend with `Axis::Custom(line)`.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs::inspector_sculpt`
   — gains 3 new brush cards.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-37-flatten-erode-ramp
./devlog/log.sh new sprint-37-symmetry-line-picker
```

## Step 3 — Scope

One commit per item.

### 1. Flatten brush

`crates/barme-core/src/brushes/flatten.rs` (new):

```rust
pub struct FlattenBrush {
    target_height: u16,  // sampled at stamp center on first stamp
}

impl Brush for FlattenBrush {
    fn id() -> &'static str { "flatten" }
    fn apply(&self, hm: &mut Heightmap, stamp: BrushStamp) -> DirtyRect {
        for z in min_z..max_z {
            for x in min_x..max_x {
                let d = distance(x, z, stamp.center);
                if d > stamp.radius { continue; }
                let falloff = falloff_quad(d / stamp.radius);
                let current = hm[(z, x)];
                let blended = lerp(current, self.target_height, falloff * stamp.strength);
                hm[(z, x)] = blended;
            }
        }
        DirtyRect { ... }
    }
}
```

`target_height` samples the heightmap at the brush center on the
FIRST stamp of a drag — subsequent stamps reuse it for consistent
flattening. Inspector exposes a "Lock target" toggle that pins it
explicitly.

### 2. Erode brush

Hydraulic erosion: simulates water flowing downhill carrying
sediment.

`crates/barme-core/src/brushes/erode.rs` (new):

```rust
pub struct ErodeBrush {
    droplets_per_stamp: u32,  // 50 default
}

impl Brush for ErodeBrush {
    fn id() -> &'static str { "erode" }
    fn apply(&self, hm: &mut Heightmap, stamp: BrushStamp) -> DirtyRect {
        let mut rng = small_rng(stamp.seed);
        for _ in 0..self.droplets_per_stamp {
            // Random drop within stamp radius.
            let drop_x = stamp.center.x + rng.gen_range(-stamp.radius..=stamp.radius);
            let drop_z = stamp.center.z + rng.gen_range(-stamp.radius..=stamp.radius);
            simulate_droplet(hm, drop_x, drop_z, stamp.strength);
        }
        DirtyRect { ... }
    }
}

fn simulate_droplet(hm: &mut Heightmap, x: f32, z: f32, strength: f32) {
    let mut pos = (x, z);
    let mut velocity = 0.0;
    let mut sediment = 0.0;
    for _step in 0..MAX_LIFETIME {
        let gradient = compute_gradient(hm, pos);
        if gradient.magnitude() < EPSILON { break; }
        let dir = gradient.normalize();
        let next = pos + dir * STEP_SIZE;

        let drop_height = hm.sample_bilinear(pos);
        let next_height = hm.sample_bilinear(next);
        let height_delta = drop_height - next_height;

        velocity = sqrt(velocity*velocity + height_delta * GRAVITY);
        let capacity = velocity * SEDIMENT_CAPACITY * strength;

        if sediment > capacity {
            // Deposit
            let deposit = (sediment - capacity) * DEPOSITION_RATE;
            hm.deposit_at(pos, deposit);
            sediment -= deposit;
        } else {
            // Erode
            let erosion = min((capacity - sediment) * EROSION_RATE, height_delta);
            hm.erode_at(pos, erosion);
            sediment += erosion;
        }

        pos = next;
    }
}
```

Implementation references:
- Hans Beyer's "Implementation of a method for hydraulic
  erosion" (game-dev common reference).
- Sebastian Lague's erosion video (YouTube) — well-known
  implementation reference; pseudocode applicable.

Erosion is computationally heavier than raise/lower/smooth.
Wrap the droplet loop in `rayon::par_iter` (each droplet is
independent except for the heightmap writes — use atomic-add
on tile values or per-thread heightmap-delta buffers that
merge at end-of-stamp).

### 3. Ramp brush

A line-tool: click + drag to define a line, the heightmap
linearly interpolates from start to end height.

`crates/barme-core/src/brushes/ramp.rs` (new):

```rust
pub struct RampBrush {
    line_width: f32,
    start_pos: Option<(f32, f32)>,  // captured on drag start
}

impl Brush for RampBrush {
    fn id() -> &'static str { "ramp" }
    fn apply(&self, hm: &mut Heightmap, stamp: BrushStamp) -> DirtyRect {
        if self.start_pos.is_none() {
            // First stamp: capture start. Don't modify.
            return DirtyRect::none();
        }
        let start = self.start_pos.unwrap();
        let end = (stamp.center.x, stamp.center.z);
        let start_h = hm.sample_bilinear(start);
        let end_h = hm.sample_bilinear(end);

        // For each pixel within line_width of the segment...
        for z in z_range {
            for x in x_range {
                let t = project_onto_line(x, z, start, end);
                if t < 0.0 || t > 1.0 { continue; }
                let perp_dist = distance_to_line(x, z, start, end);
                if perp_dist > self.line_width { continue; }
                let target = lerp(start_h, end_h, t);
                let blend = falloff_quad(perp_dist / self.line_width) * stamp.strength;
                hm[(z, x)] = lerp(hm[(z, x)], target, blend);
            }
        }
        DirtyRect { ... }
    }
}
```

UX: in Sculpt mode with Ramp selected, LMB-drag draws the line.
Release commits the brush. Symmetry replicates the line.

### 4. Inspector cards

`inspector_sculpt` gains 3 cards: Flatten (warn-amber colour),
Erode (danger-red? muted? pick one), Ramp (accent-blue).

Tooltip + help-text per Sprint 19 / U1 catalogue. Each
brush gets a HelpId variant.

### 5. Arbitrary-axis symmetry line picker

`crates/barme-core/src/symmetry.rs`:

```rust
pub enum SymmetryAxis {
    Horizontal,       // existing
    Vertical,
    DiagonalForward,
    DiagonalBackward,
    Quad,
    Rotational { fold: u8 },
    Custom { p1: Vec2, p2: Vec2 },  // NEW
}
```

Custom axis: a line through two points in elmos. Mirror replicates
across that line.

**UI**: `inspector_*` global symmetry chip gets a new ComboBox
option "Custom Line". Selecting it:
1. Top-bar Symmetry chip shows "Custom: (drag canvas to set)".
2. The user clicks two points on the canvas to define the axis.
3. The axis renders as a dashed yellow line.
4. Mirroring activates.

Stored in `Project.symmetry = SymmetryAxis::Custom { p1, p2 }`.

### 6. Tests + rollup

- **Flatten determinism**: same target → same output.
- **Erode determinism with fixed seed**: per-droplet PRNG is
  seeded; runs are reproducible.
- **Ramp accuracy**: pin a fixture with known start/end heights;
  verify linear interpolation.
- **Custom axis mirror**: pin a fixture; reflect across
  `(0,0)-(100,100)` diagonal; verify pixel values match
  expected.
- **F9 form Custom axis serialisation**: round-trip via TOML.

Rollup commit: STATUS UPDATEs in SRS / ROADMAP (F2 + F3
fully closed). closing devlog logs. "Sprint 38 = (user's
choice — F-import or F-procgen-v2 or F-typemap or F-asset-library)"
handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `trace!` on per-droplet sim
(behind a feature flag); `info!` on ramp commit; `trace!` on
custom-axis mirror.

## Step 5 — Out of scope

- **Per-brush settings persistence across sessions** — defer.
- **Advanced erosion params** (thermal erosion, multi-pass) —
  Stage 2 polish.
- **Symmetry that crosses map boundaries** — custom axis
  outside the map is allowed but mirror outputs may clip.
- **F-import / F-typemap / F-asset-library** — future sprints.

## Step 6 — Critical pitfalls (read twice)

1. **Erosion is hot path**: 50 droplets × 50 steps × heightmap
   sampling = ~12500 ops per stamp. Bench on Vega 8; consider
   reducing droplets if perf bites.

2. **Erode multithreading**: per-droplet heightmap writes can
   conflict. Use per-thread delta buffers that merge at end-
   of-stamp via add. Test for determinism.

3. **Ramp drag UX**: the user drags from start to end. The
   first stamp captures start; subsequent stamps re-target end.
   On release, the brush "commits" — the final end point is
   the stamp.

4. **Symmetry: custom axis edge cases**: a horizontal line at
   y=0 is the same as `Horizontal`. Detect and degrade.

5. **Custom axis serialisation**: TOML stores `p1` / `p2` as
   `[f32; 2]`. Update `Project::after_load_migrate` if
   `SCHEMA_V` bumps.

6. **Don't break existing symmetry**: H/V/Quad/Diagonal/Rotational
   all keep working. Custom is an addition.

7. **Visual feedback for custom axis pick**: while picking,
   the canvas shows a thin yellow line under the cursor (from
   p1 to mouse). On second click, p2 is set.

8. **Undo through new brushes**: each brush stamp emits a
   single dirty-rect; ADR-033's undo machinery handles them
   like any other. Verify Ctrl+Z works.

9. **Brush count cap**: Sprint 23 / FINDINGS may grow this
   when more brushes ship. Bench compile times. 6 brushes
   total is fine.

10. **Lint rule update (Sprint 21)**: if any lint rule keys on
    brush count, update. Probably none do.

## Step 7 — Exit criteria

- 5+ commits on `main`: flatten, erode, ramp, custom symmetry
  axis, inspector wiring + tests + rollup.
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (F2 + F3 fully closed).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - All 6 sculpt brushes available (raise / lower / smooth /
    flatten / erode / ramp).
  - Custom symmetry axis pickable from canvas; mirror works.
  - Erode produces visibly natural ravines on a parabolic
    bowl heightmap.
  - Ramp produces clean linear slopes.
- Final devlog: summary + future-sprint suggestions (F13,
  F14, F15, F16, F17, F21, F23, L2).

Start with flatten (simplest). Then ramp (UX-heavy). Erode
last (perf-heavy). Symmetry custom axis is independent and
can ship in parallel.
