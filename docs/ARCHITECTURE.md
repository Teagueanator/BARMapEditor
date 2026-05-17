# Architecture

Implements SRS §3.4. This document is the bridge between that diagram and the
actual crate layout.

## Crate map

```
barme-app           UI shell. egui top/side/central panels, file dialogs,
                    keybindings, autosave timer. Owns the Project. No business
                    logic — everything delegated to barme-core.

barme-core          Project model + invariants. Pure Rust, no GPU, no UI, no
                    subprocess. Serializable to TOML. Heavily tested.
                      ├─ project::Project
                      ├─ map_size::MapSize       (the 64·N+1 invariant lives here)
                      ├─ heightmap::Heightmap    (tiled COW, R16 internal)  ← TODO
                      ├─ overlays::{Metal,Type,Grass,SplatDistr}            ← TODO
                      ├─ features::FeatureList                              ← TODO
                      └─ undo::UndoStack         (per-tile deltas)          ← TODO

barme-render        (planned, Stage 0) wgpu pipeline.
                      ├─ heightmap mesh tessellator
                      ├─ DNTS shader approximation
                      ├─ feature instancing
                      └─ minimap renderer

barme-pipeline      (planned, Stage 0) PyMapConv driver + 7z packager.
                      ├─ pymapconv::compile(project_dir) -> sd7_path
                      ├─ packager::sd7_nonsolid(...)
                      └─ launcher::launch_in_bar(sd7_path)

barme-mapinfo       (planned, Stage 1) Lua AST + serializer + linter for
                    mapinfo.lua. Each pitfall in PITFALLS.md §6 is a lint rule.
```

The split mirrors the SRS architecture diagram (§3.4). Each box in that
diagram maps to exactly one crate (or, for `Project Model` + `Map Data Core`,
both into `barme-core` — they are too coupled to separate cheaply).

## Data flow (SRS §3.5, terrain edit → playable)

1. UI emits `BrushStroke { world_xz, radius, power, mode }`.
2. `barme-render` dispatches a wgpu compute shader that mutates the tiled
   R16 heightmap texture *in place*. Affected tiles marked dirty in the CPU
   mirror held by `barme-core`.
3. Symmetry post-pass replays the stroke into mirrored tiles.
4. Preview mesh tessellation reads the heightmap as a GPU texture every frame.
5. **Save:** dirty CPU tiles flushed to project file.
6. **Build:** PNG export → `barme-pipeline::pymapconv::compile` → 7z
   non-solid → `mymap.sd7`.

## Coordinate convention (single source of truth)

Spring/Recoil is **Y-up, left-handed**. We keep this internally — no
conversion to Y-down or right-handed. The only conversion point is the
heightmap *image* axis (image Y-down → world Z-positive).

| Axis | Direction | Units |
|------|-----------|-------|
| X | east | elmos (= world units) |
| Y | up | elmos (height) |
| Z | south (image-Y) | elmos |

- Heightmap pixel `(px, py)` → world `(px · 8, h, py · 8)`
- Metal/type pixel `(px, py)` → world `(px · 16, _, py · 16)`
- Feature `{x, z, rot}` is in elmos directly

`barme-core::coords` (TODO) is the only module allowed to do these
conversions. UI and render layers go through it.

## Why this split

- **`barme-core` is pure Rust with no GPU or process deps**, so it can be
  unit-tested without a display server and reused if someone wants a CLI
  build later.
- **`barme-app` knows only about egui and barme-core**, so swapping UI
  toolkits (or building a TUI) doesn't touch the model.
- **`barme-pipeline` is the only crate that ever spawns a subprocess.** If
  PyMapConv is replaced with the native fallback (SRS pivot threshold),
  it's a single-crate change.
