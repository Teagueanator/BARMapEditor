# Sprint 44 — F17 Pathability overlay

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 44** — implements **F17** (pathability overlay)
from the SRS Stage-2 list. The user can visualise which units can
traverse which parts of the map.

BAR units have **locomotor classes**: tank, bot, hover, ship,
amphibious. Each class has a max-slope-it-can-climb. The pathability
overlay computes, for each pixel:

1. The slope at that pixel (from the heightmap).
2. Which locomotor classes can traverse (slope < max for class).
3. Colour-codes per locomotor:
   - All can traverse: neutral.
   - Bots only: yellow.
   - Bots + tanks: orange.
   - Hover only (water + steep): blue.
   - Ships only (water + deep): dark blue.
   - Nothing can traverse: red.

After this sprint, mappers see at a glance which areas of their
map are tank-pathable vs bot-only vs blocked.

**Prerequisites:**
- Sprint 42 (typemap editor) — terrain types add per-locomotor
  speed multipliers that affect pathability.
- Sprint 22 (help center) — overlay help article.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F17.
3. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — `TerrainType` move_speeds.
4. Reference: BAR's unit definitions for locomotor max-slope.
   Typical values:
   - tank: 0.4 (radians)
   - bot: 0.6
   - hover: 0.8
   - ship: 0.0 (only water)
   - amphibious: 0.5

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-44-pathability-overlay
```

## Step 3 — Scope

### 1. Pathability compute

`crates/barme-core/src/pathability.rs` (new):

```rust
pub struct Pathability {
    pub mask: Vec<u8>,  // bitmask: 0x01 = tank, 0x02 = bot, 0x04 = hover, 0x08 = ship
    pub dim: (u32, u32),
}

pub fn compute_pathability(
    heightmap: &Heightmap,
    type_map: &TypeMap,
    terrain_types: &[TerrainType],
    water_mode: WaterMode,
) -> Pathability;
```

For each pixel:
1. Compute slope = max(|grad x|, |grad z|) over a 3×3 window.
2. Check water (heightmap pixel < water_plane_y → only hover/ship).
3. Apply terrain_types[type_id].move_speeds multiplier — speed = 0
   blocks the locomotor.
4. Bitmask the result.

Use rayon for the per-row loop (Sprint 24's pattern).

### 2. Overlay rendering

`crates/barme-app/src/render.rs`: a new pipeline that takes the
pathability mask + draws it as a translucent texture overlay on
the terrain.

Activated via `View > Pathability overlay` menu item or
`Ctrl+P`. Toggle on/off; preserves last state per-project.

Colour map (config in `theme.rs`):
- All pathable: `Color32::TRANSPARENT_BLACK` (no tint).
- Tank+bot+hover: `Color32::from_rgba_premultiplied(0, 255, 0, 64)`
  (faint green).
- Bot+hover only: `Color32::from_rgba_premultiplied(255, 255, 0, 64)`
  (yellow).
- Hover only: `Color32::from_rgba_premultiplied(0, 100, 255, 64)`
  (blue).
- Ship only: `Color32::from_rgba_premultiplied(0, 0, 255, 64)`
  (dark blue).
- Nothing: `Color32::from_rgba_premultiplied(255, 0, 0, 96)` (red).

### 3. Legend + chip in status strip

When the overlay is active, status strip shows a small legend
chip: "Pathability: ●green ●yellow ●blue ●red". Hover shows the
full key.

### 4. Re-bake trigger

Pathability is computed when:
- Heightmap changes (any sculpt commit).
- Type map changes.
- Terrain_types edited via F9 form.
- Water mode changes.

Debounced 500 ms after the last change. Async on worker thread
(reuse Sprint 24 pattern) since 16-SMU compute is ~200 ms.

### 5. Tests + rollup

- **Pathability compute**: pin a fixture heightmap with a known
  slope → assert mask values match expected per-locomotor.
- **Water blocks land**: pixel below water_plane → mask = 0x0c
  (hover + ship only).
- **Toggle UI**: Ctrl+P toggles overlay; visual smoke.
- **Rollup**: STATUS UPDATEs (F17 done; Stage 2 F-list close to
  done).

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on pathability bake with
timing; `trace!` on overlay toggle.

## Step 5 — Out of scope

- **A* path-finding visualisation** (showing a path from A to B)
  — Stage 2 polish.
- **Custom locomotor classes** beyond the BAR standard 5 —
  out of scope.
- **Animated overlay** (showing unit movement preview) — Stage 3.

## Step 6 — Critical pitfalls

1. **Slope computation**: use central differences, not forward.
   Forward differences bias the slope toward one direction.

2. **Water plane y**: read from `Project.mapinfo.water.plane_y`
   (typically 0). If water mode == None, treat as -infinity (all
   land).

3. **Bitmask packing**: 4 bits suffice for 4 locomotor classes;
   use the upper 4 for future classes (amphibious, deep-water-
   only, etc.).

4. **Overlay vs terrain depth**: render overlay AFTER terrain
   but BEFORE water. Depth-test pass, depth-write off (translucent).

5. **Frame budget**: overlay is a single textured quad sample
   per fragment; negligible perf cost.

## Step 7 — Exit criteria

- 4+ commits on `main`: pathability compute, overlay pipeline,
  toggle UI + legend, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (F17 done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: load a hilly fixture → Ctrl+P → overlay reveals
  bot-only / tank-blocked / water-only regions. Edit terrain →
  overlay re-bakes within 500ms.
- Final devlog: summary + "Sprint 45 = F21 theme toggle + F22
  live status" handoff.
