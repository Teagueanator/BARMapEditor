# Layered painter (reference)

Cross-tool reference for the Sprint 15–17 painter. Most users
arrive here via the **Paint layer** tool article — this article
covers the data model and bake side.

## Data model

The project carries a `LayerStack: Vec<TextureLayer>`. Each
`TextureLayer` has:

- `id` — stable string (UUID-ish), keyed across save/load.
- `source` — `Stock(slot_idx)` or `Imported(png_bytes,
  thumbnail_bytes)`.
- `mask` — tiled 256×256 COW alpha buffer (`MaskTiles`).
- `blend_mode` — Normal / Multiply / Add (currently Normal-only
  in the bake; the inspector exposes the others for forward
  compat).
- `opacity` — 0..1 global multiplier.
- `visible` — toggleable per-frame without invalidating the
  cache.

## Composite

The GPU composite renders the stack into a `Rgba8UnormSrgb` RT
every frame the stack changes. Each frame's `prepare()`
re-uploads only the dirty mask tiles (per-layer `version()`
counter; see ADR-040). The composite is what the canvas displays
and what bakes into the diffuse BMP.

## Bake

On Build:

1. The stack composites once into a CPU side buffer (`image`
   crate, RGBA8).
2. Per-active DNTS channel (slot bound + non-zero mask) the
   pipeline bakes a `*.dds` via the D2 `bake_dnts` helper.
3. The splat distribution PNG (1024², RGBA) is written from the
   per-channel coverage.
4. `mapinfo.resources.splatDetailNormalTex` is emitted in the
   subtable form (PITFALL §15).

## Migration from legacy splat config

Pre-Sprint-15 projects carried a `splat_config` block with
explicit per-channel slot bindings + `tex_scales` / `tex_mults`
/ `diffuse_in_alpha`. `Project::after_load_migrate` seeds the
new stack from this block; the one-time toast informs the user.
The field is `#[serde(skip)]` so saves never emit it; D10 / ADR-041
also dropped the legacy `Tool::SplatPaint`.
