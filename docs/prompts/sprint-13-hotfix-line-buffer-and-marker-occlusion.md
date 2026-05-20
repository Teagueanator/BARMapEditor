# Sprint 13 hotfix — line buffer overflow + marker terrain occlusion

You are continuing work on the BAR Map Editor (Rust + egui + wgpu desktop
app). Sprint 13 (ADR-037 — offscreen render target + GPU markers + line
pipeline) shipped on `main` across seven commits ending with `7d9cc3d`.
A live smoke test surfaced two issues that need surgical fixes before
the next sprint. Both are isolated to `crates/barme-app/src/`.

This is a **hotfix sprint**: two small commits, each with tests + a
brief devlog. No new ADR; STATUS UPDATE on ADR-037 only.

## Step 1 — read the context

Read in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/docs/DECISIONS.md` — find **ADR-037**
   (search for "Offscreen render target + GPU markers + line pipeline").
   Understand the marker pipeline + line pipeline design, the
   `MARKER_Y_LIFT_ELMOS = 2.0` constant, and the
   `LINE_VERTEX_CAPACITY = 5_000` buffer cap.
3. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs` —
   focus on:
   - `OFFSCREEN_CLAMP`, `MARKER_INSTANCE_CAPACITY`, `LINE_VERTEX_CAPACITY`
     constants near the top.
   - `LineVertex` struct + `install_line_resources`.
   - `TerrainCallback::prepare` (the warn about "line vertex buffer
     exceeded" lives here).
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/overlay.rs`
   — `collect_symmetry_segments`, `dash_subsegments`, `DASH_ON_PX`,
   `DASH_OFF_PX`, `MIN_DASHED_LENGTH_PX`.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/markers.rs`
   — `MARKER_Y_LIFT_ELMOS` + `MarkerBatch::into_instances` (the Y-lift
   is applied at encode time).
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs` —
   `central()` (starts around line 6140). Look for the **PHASE A**
   block that builds the marker batch (start positions, metal spots,
   geo vents). Each iteration constructs world positions like
   `glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32)` —
   note the hard-coded `0.0` for the Y component.
7. `crates/barme-core/src/heightmap.rs` — find the `Heightmap` struct
   and its `data()` / `dims()` accessors. This is the CPU-side
   heightmap the App holds in `self.heightmap`.
8. `crates/barme-app/src/main.rs` again — find the `App` struct's
   `heightmap: Option<Heightmap>` field and `height_scale: f32` field
   (the max-height multiplier — search for "height_scale").
9. `crates/barme-app/src/render.rs::ELMOS_PER_PIXEL` (= 8.0) — the
   world ↔ heightmap pixel scale.

## Step 2 — devlog flow

```bash
./devlog/log.sh new stage-1-renderer-depth-rework-hotfix
./devlog/log.sh log stage-1-renderer-depth-rework-hotfix "starting"
```

Fill `goals.md` with the two definitions of done below. Write a
session log under `logs/` per commit.

## Step 3 — Bug 1: marker terrain occlusion (most important)

### Symptom (verbatim from user testing 2026-05-19)

> "I can't see the metal or geo vent features unless I'm looking UNDER
> the map. They're completely occluded."

User's repro: F1 wizard → "Parabolic bowl" biome with `max_height ≈
1236` (a dome in the centre of the map). Place metal spots near map
centre (e.g. `(4329, 4922)`). Markers don't show from above; they
appear only when the camera is BELOW the terrain.

### Root cause

`crates/barme-app/src/main.rs::central` PHASE A constructs marker
world positions as `glam::Vec3::new(x_elmo, 0.0, z_elmo)` — Y is
hard-coded to 0. `MarkerBatch::into_instances` then applies
`MARKER_Y_LIFT_ELMOS = 2.0`, giving a final world Y of **2 elmos**.

For a "Flat plain" map (`expr=0.0`) this is fine — terrain is at Y=0,
markers are at Y=2, depth-test passes. For ANY map with relief at
that XZ, the terrain Y can be hundreds or thousands of elmos. The
marker at Y=2 is BELOW the terrain → the terrain pipeline's depth
write rejects the marker via the marker pipeline's
`depth_compare: Less`.

This is a Sprint-13 oversight. The `MARKER_Y_LIFT_ELMOS` constant
was sized to fix h=0 z-fight, not to clear arbitrary terrain
elevations.

### Fix

Sample the heightmap on the CPU side at the marker's XZ and use the
terrain height (in world units) as the marker's world Y. The existing
`MARKER_Y_LIFT_ELMOS` continues to add the small epsilon on top — so
markers sit just above the surface regardless of elevation.

**File: `crates/barme-app/src/main.rs`** (add a helper method on
`App`, in one of the existing `impl App` blocks):

```rust
/// World-space Y of the terrain surface at `(x_elmo, z_elmo)`.
/// Returns 0.0 if no heightmap is loaded. Used by Sprint-13
/// marker construction to lift markers onto the terrain surface
/// (Sprint 13 hotfix — markers at world y=0 were occluded by
/// any non-flat terrain).
///
/// Pixel rounding: nearest-neighbour. Heightmap is at
/// `ELMOS_PER_PIXEL` = 8 elmos/pixel; sub-pixel accuracy isn't
/// load-bearing for marker placement.
fn terrain_y_at(&self, x_elmo: f32, z_elmo: f32) -> f32 {
    let Some(hm) = self.heightmap.as_ref() else {
        return 0.0;
    };
    let (w, h) = hm.dims();
    if w == 0 || h == 0 {
        return 0.0;
    }
    let px = ((x_elmo / render::ELMOS_PER_PIXEL).round() as i32)
        .clamp(0, w as i32 - 1) as u32;
    let pz = ((z_elmo / render::ELMOS_PER_PIXEL).round() as i32)
        .clamp(0, h as i32 - 1) as u32;
    let raw = hm.data()[(pz as usize) * (w as usize) + (px as usize)];
    (raw as f32 / 65535.0) * self.height_scale
}
```

Then update every marker-construction site in PHASE A of `central()`
to use the helper. Sites to change (search `glam::Vec3::new(pos.x_elmo
as f32, 0.0` and the similar `spot.x_elmo` / `vent.x_elmo` patterns):

- Start-position primary + mirrors (`pos.x_elmo`, `pos.z_elmo`).
- Metal-spot primary + extractor radius ring + mirrors (`spot.x_elmo`,
  `spot.z_elmo`).
- Geo-vent primary + outline-triangle mirrors (`vent.x_elmo`,
  `vent.z_elmo`).
- Geo-vent plume (the `line_vertices` push — `base` and `top`).
- Brush ring world positions (already computed from `screen_to_world_y0`
  which returns a y≈0 position; lift these too, OR use the cursor's
  raycast y).

Pattern to replace:
```rust
let world = glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32);
```
With:
```rust
let y = self.terrain_y_at(pos.x_elmo as f32, pos.z_elmo as f32);
let world = glam::Vec3::new(pos.x_elmo as f32, y, pos.z_elmo as f32);
```

For the symmetry mirrors (where the position is `(mx, mz)` from
`replicate`), do the same with `self.terrain_y_at(mx, mz)`.

For the geo-vent plume in the PHASE A line-vertex section:
```rust
let base_y = self.terrain_y_at(vent.x_elmo as f32, vent.z_elmo as f32);
let base = glam::Vec3::new(
    vent.x_elmo as f32,
    base_y + crate::ui::markers::MARKER_Y_LIFT_ELMOS,
    vent.z_elmo as f32,
);
let top = base + glam::Vec3::new(0.0, PLUME_HEIGHT_ELMOS, 0.0);
```
(`MARKER_Y_LIFT_ELMOS` here because line vertices are NOT lifted by
`into_instances` — only markers are. The plume needs its own lift.)

For symmetry axes in `overlay::collect_symmetry_segments`: the axes
currently sit at constant Y = `MARKER_Y_LIFT_ELMOS`. The right fix
is to sample terrain Y per-vertex, but that requires the heightmap to
flow into the overlay module. **Defer** — axes are a thin 1-px line
and z-fighting them is far less visible than missing markers. Leave
the existing `lift = MARKER_Y_LIFT_ELMOS` as-is for this hotfix.

### Tests for Bug 1

In `main.rs`'s existing `#[cfg(test)] mod tests` block, add:

- `terrain_y_at_returns_zero_without_heightmap` — `App` with
  `heightmap = None` returns 0.0 for any XZ.
- `terrain_y_at_samples_known_value` — construct a `Heightmap` with a
  known value at a known pixel, assert `terrain_y_at` returns
  `value * height_scale / 65535.0`.
- `terrain_y_at_clamps_out_of_bounds` — XZ way past the map extent
  doesn't panic; returns the edge value.
- `terrain_y_at_rounds_to_nearest_pixel` — XZ at sub-pixel positions
  (4.0 elmos, 3.9 elmos) rounds correctly given `ELMOS_PER_PIXEL=8`.

Use `Heightmap::new(dims, data)` (or whatever constructor exists)
with a small fixture (e.g. 9×9 or 17×17).

### Smoke check for Bug 1

```bash
. ~/.cargo/env && cargo run -p barme-app
```

F1 wizard → "Parabolic bowl" → 16×16 SMU. Place 2 metal spots near
map centre. They should be VISIBLE from default-framing camera angle
(no need to orbit under the map). Sculpt a 200-elmo hill and place
markers on it — markers should sit on the hill surface, not at the
ground plane.

Verify the depth-occlusion behaviour still works correctly: place a
marker behind a hill, orbit so the hill is between camera and marker
— the hill should still hide the marker (the fix lifts markers to
terrain surface, not above EVERY terrain).

## Step 4 — Bug 2: line vertex buffer overflow

### Symptom (verbatim)

```
WARN barme::render: line vertex buffer exceeded; tail dropped requested=13306 capacity=5000
```

Repeats hundreds of times while the user is orbiting / panning. The
spike to 13306 suggests dashed symmetry axes are producing thousands
of dashes per frame at certain camera angles.

### Root cause

`overlay::collect_symmetry_segments` (in `ui/overlay.rs`) projects
both axis endpoints to screen space, then calls `dash_subsegments(a,
b)` to generate 8-px-on / 4-px-off sub-segments along the projected
screen distance.

At extreme zoom-in, the projected screen distance between the axis
endpoints can be MASSIVE — both endpoints can be far off-screen with
the entire axis crossing the visible rect. A 16-SMU axis projected at
high zoom can span hundreds of thousands of pixels; `dash_subsegments`
emits one sub-segment per 12 px → 10k+ dashes → 20k+ vertices, well
past the 5 000-vertex pre-allocated buffer.

The user repro: orbit / zoom-in while symmetry is set to Horizontal
or Vertical. The warn-spam starts as soon as the projected axis
length crosses ~5 000 / 2 / 12 ≈ 200 dashes.

### Fix

Two-part fix:

1. **Clip the axis endpoints to a "visible-plus-margin" screen rect
   before dashing.** This bounds the dash count by the rect width
   (the projected on-screen portion of the axis), not by the
   full-axis projected length. Off-screen dashes wouldn't render
   anyway (clipped by the GPU rasterizer).
2. **Hard cap the dashes per axis at a sensible ceiling** (e.g. 256)
   so even pathological projections can't overflow. Drop to a solid
   line when the cap kicks in (still informative; user just doesn't
   see dashes when zoomed in to the point that individual dashes
   wouldn't be visible anyway).

**File: `crates/barme-app/src/ui/overlay.rs`**

Add a helper that clips a 2D segment to a rect (axis-aligned, in
screen space):

```rust
/// Liang–Barsky-style clip of segment `(a, b)` to the axis-aligned
/// screen rect `[0, rect_size.x] × [0, rect_size.y]` expanded by
/// `margin_px` on each side. Returns `Some((a', b'))` with both
/// endpoints inside the expanded rect, or `None` if the entire
/// segment lies outside.
///
/// Sprint 13 hotfix: bounds the dash count in
/// `collect_symmetry_segments` when the camera is zoomed in far
/// enough that the projected axis spans hundreds of thousands of
/// pixels.
fn clip_segment_to_rect(
    a: egui::Pos2,
    b: egui::Pos2,
    rect_size: glam::Vec2,
    margin_px: f32,
) -> Option<(egui::Pos2, egui::Pos2)> {
    // Implement Liang–Barsky. Reject if both endpoints share a
    // half-plane outside the rect; clip otherwise.
    let x_min = -margin_px;
    let y_min = -margin_px;
    let x_max = rect_size.x + margin_px;
    let y_max = rect_size.y + margin_px;
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t_enter = 0.0_f32;
    let mut t_exit = 1.0_f32;
    let clip = |p: f32, q: f32, t_enter: &mut f32, t_exit: &mut f32| -> bool {
        if p.abs() < 1e-6 {
            return q >= 0.0;
        }
        let t = q / p;
        if p < 0.0 {
            if t > *t_exit { return false; }
            if t > *t_enter { *t_enter = t; }
        } else {
            if t < *t_enter { return false; }
            if t < *t_exit { *t_exit = t; }
        }
        true
    };
    if !clip(-dx, a.x - x_min, &mut t_enter, &mut t_exit) { return None; }
    if !clip( dx, x_max - a.x, &mut t_enter, &mut t_exit) { return None; }
    if !clip(-dy, a.y - y_min, &mut t_enter, &mut t_exit) { return None; }
    if !clip( dy, y_max - a.y, &mut t_enter, &mut t_exit) { return None; }
    Some((
        egui::Pos2::new(a.x + dx * t_enter, a.y + dy * t_enter),
        egui::Pos2::new(a.x + dx * t_exit,  a.y + dy * t_exit),
    ))
}
```

Then update `collect_symmetry_segments` to:
1. Project both endpoints (existing).
2. Clip the projected pair to the rect (with ~64-px margin so a dash
   that straddles the edge isn't missing one half).
3. Convert clipped screen endpoints back to interpolation parameters
   (`t_start = clipped_a relative position on a→b`, same for
   `t_end`).
4. Pass clipped points to `dash_subsegments`.
5. The world-space lerp continues to use the ORIGINAL `a_world` /
   `b_world` with the new `t_start` / `t_end` values.

Existing code (for reference, before the change):
```rust
let a_pt = egui::Pos2::new(a_s.x, a_s.y);
let b_pt = egui::Pos2::new(b_s.x, b_s.y);
let total = (b_pt - a_pt).length();
if total < 1e-3 { continue; }
for (s_start, s_end) in dash_subsegments(a_pt, b_pt) {
    let t_start = ((s_start - a_pt).length() / total).clamp(0.0, 1.0);
    let t_end = ((s_end - a_pt).length() / total).clamp(0.0, 1.0);
    let w_start = a_world.lerp(b_world, t_start);
    let w_end = a_world.lerp(b_world, t_end);
    out.push(crate::render::LineVertex::new(w_start, AXIS_COLOUR));
    out.push(crate::render::LineVertex::new(w_end, AXIS_COLOUR));
}
```

New version (sketch):
```rust
let total = (b_s - a_s).length();
if total < 1e-3 { continue; }
let Some((clip_a, clip_b)) =
    clip_segment_to_rect(
        egui::Pos2::new(a_s.x, a_s.y),
        egui::Pos2::new(b_s.x, b_s.y),
        rect_size,
        64.0, // margin so an edge-straddling dash isn't half-missing
    )
else {
    continue; // segment entirely off-screen
};
// Hard cap dashes (one cycle ≈ DASH_ON_PX + DASH_OFF_PX = 12 px).
let clip_len = (clip_b - clip_a).length();
const MAX_DASHES_PER_AXIS: usize = 256;
let predicted_dashes =
    ((clip_len / (DASH_ON_PX + DASH_OFF_PX)) as usize).max(1);
if predicted_dashes > MAX_DASHES_PER_AXIS {
    // Solid fallback — emit a single segment in world space.
    let t_start = ((clip_a - egui::Pos2::new(a_s.x, a_s.y)).length()
                   / total).clamp(0.0, 1.0);
    let t_end = ((clip_b - egui::Pos2::new(a_s.x, a_s.y)).length()
                 / total).clamp(0.0, 1.0);
    let w_start = a_world.lerp(b_world, t_start);
    let w_end = a_world.lerp(b_world, t_end);
    out.push(crate::render::LineVertex::new(w_start, AXIS_COLOUR));
    out.push(crate::render::LineVertex::new(w_end, AXIS_COLOUR));
    continue;
}
for (s_start, s_end) in dash_subsegments(clip_a, clip_b) {
    let t_start = ((s_start - egui::Pos2::new(a_s.x, a_s.y)).length()
                   / total).clamp(0.0, 1.0);
    let t_end = ((s_end - egui::Pos2::new(a_s.x, a_s.y)).length()
                 / total).clamp(0.0, 1.0);
    let w_start = a_world.lerp(b_world, t_start);
    let w_end = a_world.lerp(b_world, t_end);
    out.push(crate::render::LineVertex::new(w_start, AXIS_COLOUR));
    out.push(crate::render::LineVertex::new(w_end, AXIS_COLOUR));
}
```

The `clamp(0.0, 1.0)` ensures world positions stay along the actual
world-space axis even if numerical noise pushes `t` slightly past
the endpoint.

**Bonus, in `render.rs`**: bump `LINE_VERTEX_CAPACITY` from 5_000 to
8_000 as a belt-and-suspenders measure (256 max dashes × 2 verts ×
4 axes (worst-case Quad symmetry) = 2 048 verts — comfortably under
8 000 with margin for geo-vent plumes and future line uses). The
warn-spam goes away because the cap prevents the overflow at its
source, not just because the buffer is bigger.

### Tests for Bug 2

In `crates/barme-app/src/ui/overlay.rs`'s existing
`#[cfg(test)] mod tests`:

- `clip_segment_to_rect_passes_fully_inside` — segment inside the
  rect returns Some with both endpoints unchanged.
- `clip_segment_to_rect_clips_one_endpoint_outside` — segment with
  endpoint outside returns Some with the outside endpoint moved to
  the rect edge.
- `clip_segment_to_rect_rejects_fully_outside` — segment outside
  the rect returns None.
- `clip_segment_to_rect_passes_segment_straddling_two_edges` —
  segment with both endpoints in opposite "quadrants" outside the
  rect (but the segment line itself crosses the rect) returns Some
  with both endpoints on rect edges.
- `clip_segment_to_rect_respects_margin` — margin actually expands
  the accept region.
- `collect_symmetry_segments_caps_dashes_when_zoomed_in` — construct
  a camera with a small `distance` (so the axis projects across many
  pixels), call `collect_symmetry_segments`, assert the output vertex
  count is bounded (≤ `MAX_DASHES_PER_AXIS * 2` for one axis).

### Smoke check for Bug 2

```bash
. ~/.cargo/env && cargo run -p barme-app
```

F1 wizard → Horizontal or Quad symmetry. Orbit the camera and zoom
in HARD (mouse wheel down all the way, then more). The tracing log
should show **zero** "line vertex buffer exceeded" warns. The
symmetry axes should still be visible at every zoom level (dashed
when reasonable, solid line when zoomed in to the point that dashes
would be sub-pixel).

## Step 5 — standing constraints

- `source ~/.cargo/env`.
- After each commit: `cargo fmt && cargo clippy --workspace --all-targets
  -- -D warnings && cargo test --workspace`. All green required.
- Terse commit subjects. **No** `Co-Authored-By: Claude`. Local-only
  (no push) unless asked.
- One devlog folder for both fixes: `stage-1-renderer-depth-rework-hotfix`.

## Step 6 — exit criteria

- 2 commits on `main`:
  - Commit 1: `markers: lift to terrain surface (Sprint 13 hotfix)`
  - Commit 2: `lines: clip + cap symmetry-axis dashes (Sprint 13 hotfix)`
- Devlog folder filled (`goals.md`, `notes.md`, 2 session logs).
- ADR-037 in `docs/DECISIONS.md` gets a one-paragraph STATUS UPDATE
  appended noting the two hotfixes (don't rewrite the ADR; just
  append "STATUS UPDATE 2026-05-19 (hotfix):" at the end of the
  Consequences section).
- New tests added per the per-bug test list above; existing tests
  stay green.
- Manual smoke per the two "Smoke check" sections above.

## Step 7 — critical pitfalls (read twice)

1. **`self.heightmap` is `Option<Heightmap>`.** `terrain_y_at` must
   return 0.0 on `None` (the "no heightmap loaded" state). Don't
   panic.
2. **`Heightmap::data()` returns `&[u16]`** in row-major order
   (`pz * w + px`). The `dims()` returns `(width, height)`. Mixing
   up the index produces silently-wrong heights at non-square maps.
3. **Heightmap pixel coords scale from world by `ELMOS_PER_PIXEL =
   8.0`**, NOT by the map's SMU size. `1 elmo = 1/8 px`. Search
   `render.rs` for `ELMOS_PER_PIXEL` to confirm.
4. **`height_scale` IS the world-space max height** (its name is
   misleading — it's not a scale factor in [0,1]; it's elmos at
   heightmap value 65535). Look at the terrain shader's
   `vertex_index` → world Y math to confirm: `y = (raw_u16 / 65535)
   * height_scale`.
5. **The marker Y-lift (`MARKER_Y_LIFT_ELMOS = 2.0`) is applied by
   `MarkerBatch::into_instances`, not at marker construction.** So
   `terrain_y_at` returns the SURFACE height; the +2 gets added on
   top by the existing pipeline. Don't add it twice.
6. **For the geo-vent plume LineVertex push (not a marker), the
   Y-lift is NOT auto-applied** — the existing code adds
   `MARKER_Y_LIFT_ELMOS` manually. After the fix, the plume's base
   Y becomes `terrain_y + MARKER_Y_LIFT_ELMOS`.
7. **Liang–Barsky clip is sensitive to division-by-zero on
   axis-aligned segments.** The `if p.abs() < 1e-6 { return q >= 0.0; }`
   guard handles it; test that vertical and horizontal axes survive.
8. **`dash_subsegments` returns empty for sub-pixel segments**
   (length < 1e-3). After clipping, a barely-on-screen axis may
   return zero segments; that's correct behaviour (we drop it).
9. **The world-space `t` interpolation lerps along the FULL axis
   `(a_world, b_world)`, not the clipped axis.** Compute `t_start`
   / `t_end` relative to the ORIGINAL `a_s` / `b_s` projection, NOT
   relative to `clip_a` / `clip_b`. Test this with a Quad-symmetry
   axis at high zoom: the dashes should align with the off-screen
   continuation if the user pans the camera.
10. **The PHASE A marker batch already runs every frame.** The
    `terrain_y_at` lookup is on the hot path; keep it cheap (no
    allocations, no logarithmic ops). A nearest-neighbour pixel
    lookup is `O(1)`; that's fine.

## Step 8 — out of scope (defer to a future cleanup)

- **Bilinear heightmap sampling for markers.** Nearest-neighbour at 8
  elmos/pixel is plenty for marker placement; bilinear adds complexity
  for no visible win.
- **Lifting symmetry axes to follow terrain.** Single 1-px lines —
  z-fighting them is far less visible than missing markers. Defer.
- **Brush ring lifted to terrain surface.** The cursor's world
  position comes from `screen_to_world_y0` (the y=0 raycast) — it's
  already y=0 + MARKER_Y_LIFT. For non-flat terrain the ring sits
  slightly above/below the surface. Acceptable for the brush since
  the user is editing terrain; revisit when GPU-side ray-vs-heightmap
  picking lands.
- **GPU-side terrain-Y lookup in the marker shader.** Would require
  binding the heightmap texture to the marker pipeline (extra bind
  group entry + uniforms). CPU lookup is simpler and the marker count
  is tiny (<100 typically).

Start by reading the existing `central()` PHASE A block and counting
the exact number of `glam::Vec3::new(*.x_elmo as f32, 0.0, *.z_elmo
as f32)` patterns to update. Bug 1's diff is mechanical once
`terrain_y_at` exists; the surface area is just "find every
hardcoded `0.0` Y in the marker push sites."
