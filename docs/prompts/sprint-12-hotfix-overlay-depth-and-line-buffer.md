# Sprint 12 hotfix — viewport overlay depth + line-vertex buffer overflow

Paste this block into a fresh Claude / Claude Code session. The prompt is
self-contained; the new agent does not need any prior conversation context.

---

You are continuing work on the BAR Map Editor at `/home/teague/code/BARMapEditor/`
(Rust workspace: `barme-core`, `barme-pipeline`, `barme-app`). Branch is `main`.
Read `CLAUDE.md` and `SRS.md` for project context. Build with
`. ~/.cargo/env && cargo run -p barme-app`. Pre-commit gate is
`cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

**Live-BAR smoke testing context.** The user has now done THREE smoke
tests of the build pipeline against actual BAR / Recoil:

- **Smoke test #1** surfaced 3 BAR-side bugs (geo path wrong, max_metal too
  low, mapinfo audit drift). Fixed in commits `f326365`, `ef19ea8`, and the
  Sprint 10 audit arc.
- **Smoke test #2** surfaced 2 BAR-side bugs (missing LuaGaia bootstrap,
  broken `map_startboxes.lua` shape/empty-file shadowing). Fixed in commit
  `43e85ce`. PITFALLS §25 and §26 capture the rules.
- **Smoke test #3 (this prompt)** surfaced **editor-side** rendering bugs.
  The user can place geo vents and metal spots in the editor, but the
  markers don't render correctly. The SD7 itself is presumably fine
  (Sprint 11 + the smoke-test-2 fixes ship the gadget trio, LuaGaia
  bootstrap, and feature-placer set.lua correctly); this hotfix is about
  the **editor viewport**, not the build pipeline.

## What the user reports

Quoting them directly:

> "I can't see anything from the top other than the metal/second values
> (2.0). The Geo vents are only visible from under the map. Those should
> be rendered on top, no?"

Screenshots (described in detail — actual PNGs may not be available to
you):

- **Screenshot 1** — low-angle 3D view (`Cam: yaw 183° pitch -25° dist 10169`).
  The user sees `2.0` text labels for several metal spots, two yellow
  triangles (start positions), one blue diamond (a different start
  position marker?), and **exactly one** cyan/teal geo-vent marker (visible
  on the right side at roughly x=6144, z=3811). Twelve geo vents are
  authored in the project (V01–V12 listed in the right-hand inspector),
  but eleven are invisible from this angle.
- **Screenshot 2** — overhead 3D view (`Cam: yaw 137° pitch 36° dist 8556`)
  showing the parabolic dome heightmap (white snow at centre, descending
  through grey/sand/water at the rim). Metal `2.0` labels visible across
  the dome surface. **No geo vents visible at all** from this angle.

User confirms by orbiting the camera below the map: all 12 geo vents
become visible from UNDER the terrain mesh. So the markers are being
rendered at the wrong **y-coordinate** AND the depth test correctly
occludes them whenever the terrain mesh is between the camera and the
marker.

## Log evidence

The user's `cargo run` session produced this excerpt — note the
hundreds of repeated `line vertex buffer exceeded` warnings:

```
INFO barme: metal spot placed x_elmo=4329 z_elmo=4922 metal=2.0 symmetry="horizontal"
INFO barme: metal spot placed x_elmo=3863 z_elmo=4922 metal=2.0 symmetry="horizontal"
...
WARN barme::render: line vertex buffer exceeded; tail dropped requested=5234 capacity=5000
WARN barme::render: line vertex buffer exceeded; tail dropped requested=5514 capacity=5000
...
WARN barme::render: line vertex buffer exceeded; tail dropped requested=13306 capacity=5000
WARN barme::render: line vertex buffer exceeded; tail dropped requested=13306 capacity=5000
   (repeats ~50 more times)
```

12 geo vents + symmetry axes pushed the line buffer from `requested=5234`
peak up to `requested=13306` peak. Sustained over many frames →
hundreds of identical warn lines, drowning out everything else.

## The three bugs

### Bug A — marker y-coordinate is hard-coded to 0.0 (root cause of the visibility issue)

All three marker types (`start_position`, `metal_spot`, `geo_vent`) push
their world position as `Vec3::new(x_elmo, 0.0, z_elmo)`. Specifically:

- `crates/barme-app/src/main.rs:6568` — start positions, `glam::Vec3::new(pos.x_elmo as f32, 0.0, pos.z_elmo as f32)`
- `crates/barme-app/src/main.rs:6611` — metal spots, `glam::Vec3::new(spot.x_elmo as f32, 0.0, spot.z_elmo as f32)`
- `crates/barme-app/src/main.rs:6668` — geo vents, `glam::Vec3::new(vent.x_elmo as f32, 0.0, vent.z_elmo as f32)`
- And the mirror entries at the equivalent symmetry-replicate blocks.

The marker pipeline at `crates/barme-app/src/render.rs:795-801` uses
`depth_compare: CompareFunction::Less` with `depth_write_enabled: false`
— so markers DO depth-test against the terrain mesh but do NOT write
depth.

On the user's procgen heightmap (`1 - (x*x + z*z)` over Centered domain
= a 1236-elmo-tall parabolic dome), markers placed at y=0 inside the
dome footprint are *below* the terrain surface, get occluded by the
mesh, and only become visible when the camera goes below the
terrain.

**Why metal spots appear "correct from the top" in screenshot 2.** This
is misleading. The `2.0` labels are 2D text painted on top of the
viewport by an egui painter loop AFTER the wgpu offscreen pass
composites — they have no depth and always show. The actual red-dot
marker (the wgpu billboard) is at y=0 just like the geo vents and IS
hidden in screenshot 2; the user only sees the labels.

**Fix direction.** Two options, document the trade-off in your devlog
before picking one:

1. **Sample the heightmap.** Look up the terrain height at the marker's
   (x_elmo, z_elmo), put the marker's world_pos.y at
   `terrain_y(x,z) + small_lift`. Cleanest. Requires CPU-side access
   to the heightmap data — `App::heightmap.as_ref().map(|h| &h.data)`
   gives you the R16 pixel buffer; convert per
   `barme_core::map_size::MapSize::heightmap_dims` and the project's
   `max_height` scale. Add a helper in `barme-core` or
   `barme-app/src/render.rs::sample_height` so the start-pos / metal /
   geo / future-feature paths all share one implementation.
2. **Disable depth test for marker pipeline** (`CompareFunction::Always`).
   Cheap. Side effect: markers will draw OVER the terrain regardless of
   where they "are" in 3D. For the editor's "I want to see all my
   spots even if a mountain is between me and them" UX this might
   actually be *desired*. But it breaks the start-of-Sprint-13
   renderer-depth-rework arc (see `docs/prompts/sprint-13-renderer-depth-rework.md`)
   which explicitly wants markers depth-tested against terrain.

Recommended path: **option 1 (sample heightmap) for permanent markers
(start positions, metal spots, geo vents) so they ride the terrain
naturally; keep option 2 in reserve for tool-active "ghost" overlays
that benefit from always-on-top behaviour.** Confirm with the user
before implementing.

### Bug B — line vertex buffer capacity exhausted

`crates/barme-app/src/render.rs:66`:

```rust
pub const LINE_VERTEX_CAPACITY: u32 = 5_000;
```

Buffer is 5000 verts × 32 B = 160 KB. The user's 12-geo-vent project
needs ~13300 verts. The overflow path at `render.rs:1741-1748` clamps
to 5000 and warns; the warning fires every frame at sustained pace.

**Fix direction.** Two options:

1. **Bump capacity** to a larger fixed size (e.g. 64_000 verts ×
   32 B = 2 MB). Simple, no allocation churn. Pin the choice with a
   comment citing "12-vent + symmetry-axes peak observed at ~13k verts,
   2x headroom for future overlays."
2. **Grow the buffer dynamically.** Track `line_vertices.capacity()`,
   reallocate the wgpu buffer when content exceeds it (with a 2x growth
   factor + ceiling to avoid pathological reallocation). More code
   but bounded memory.

Recommended: **option 1 (bump to 64k).** The constant is in one place,
the buffer is fixed-size at startup, and we don't have evidence of
needing >100k verts. A C8-style lint pass could later flag projects
approaching the cap.

Either way, the warning itself should become **rate-limited or
deduplicated** so a sustained overflow produces one log line per N
seconds, not 50 per frame. The user's logging-quality concern (next
section) is partly driven by this exact noise.

### Bug C — logging hygiene review

The user asked to "make sure we have correct logging." Look at the
session log they pasted (excerpt above): the relevant signal (project
created, metal/geo placed, tool changes) is good, but the
`line vertex buffer exceeded` warning floods the log so badly that
real warnings would be hidden.

Audit `crates/barme-app/src/` for:

- `warn!` / `error!` macros that can fire on a per-frame basis without
  rate-limiting. Add `tracing`'s rate limiter or a simple
  `Cell<Instant>` "warned-once-in-the-last-N-seconds" guard.
- `info!` macros that fire on hot paths (e.g. inside `render` loops).
  Anything at info level should be a one-time event (tool changes,
  project loads), not a per-frame thing.
- Missing `info!` for events the user would want to see — `Build &
  Install` start/end, asset bake events, etc. Don't go overboard;
  match the existing event style.

## Reproducer

1. `cargo run -p barme-app`
2. File → New project → 16×16 SMU, name "untitled"
3. F1 wizard biome: "Parabolic bowl" (this is actually a dome — the
   expression is `1 - (x*x + z*z)`)
4. Symmetry: Horizontal
5. Switch to MetalSpots tool, place ~6 metal spots
6. Switch to GeoFeatures tool, place ~6 geo vents inside the dome
   footprint
7. Orbit camera to a high-angle view (yaw 137°, pitch 36°) — note geo
   vents invisible
8. Orbit camera below the map — note geo vents visible from under
9. Watch the terminal for `line vertex buffer exceeded` warning spam

## Files to focus on

- `crates/barme-app/src/main.rs` — marker emission for start positions
  (~6552–6590), metal spots (~6610–6650), geo vents (~6660–6695)
- `crates/barme-app/src/render.rs` — marker pipeline (~785–815), line
  pipeline (~895–920), `LINE_VERTEX_CAPACITY` const at line 66,
  warn-spam site at line 1741–1748
- `crates/barme-core/src/heightmap.rs` (or equivalent) — heightmap
  sampling primitives if Bug A's option 1 needs a new helper
- `docs/PITFALLS.md` — add new entry if you discover a new BAR/Recoil
  pitfall (none expected for this hotfix; the bugs are editor-internal)
- `SRS.md` — add a STATUS UPDATE under the existing 2026-05-19
  smoke-test sections once each bug is fixed

## Output expectations

Standing constraints from `CLAUDE.md`:

- Source `~/.cargo/env` before cargo
- Pre-commit gate: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` — all green
- No Claude co-author trailer in commits
- Local-only (don't `git push` unless asked)
- Write a devlog under `devlog/stage-1-mvp/` or a new
  `devlog/stage-1-overlay-depth/` folder with a date-stamped file
  (use `./devlog/log.sh log <slug> <title>`)
- SRS STATUS UPDATE after each bug fixed
- One commit per logical fix (Bug A, Bug B, Bug C) is preferred over a
  single mega-commit, but combine if the file overlap makes the split
  artificial

Commit subjects suggested:

- `render: sample terrain height for marker world_y (start/metal/geo)`
- `render: grow LINE_VERTEX_CAPACITY 5k → 64k + rate-limit overflow warn`
- `log: rate-limit per-frame warns + audit info levels`

After each commit, ask the user to retest and confirm before moving to
the next. The user is doing the visual smoke testing; you are doing
the deterministic plumbing checks.

## Standing pitfalls (do not regress)

Re-read `docs/PITFALLS.md` before changing the renderer:

- §4 `64·N + 1` heightmap dims (don't drift)
- §11 `sundir` vs `sunDir` dual-emit (mapinfo)
- §25 LuaGaia bootstrap (don't remove from `build_sd7`)
- §26 don't ship empty `map_startboxes.lua`

None of these should be touched by this hotfix, but they're easy to
nick accidentally.
