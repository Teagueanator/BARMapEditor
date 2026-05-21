# Paint layer

The Photoshop-style layered painter — Sprints 15–17 of the
renderer-parity arc. Replaces the legacy 4-channel splat
inspector with a stack of typed `TextureLayer`s composited on
the GPU.

Pressing `L` switches the central viewport from the 3D orbit
view to a top-down 2D orthographic view of the composite render
target. Left-drag paints with the active mask brush (Reveal /
Hide / Smooth / Fill); right-drag orbits the camera; middle-drag
pans; scroll zooms.

## Layers panel

The right inspector ships a Photoshop-style Layers panel:

- Each row carries a thumbnail, visibility toggle, blend-mode
  combo, opacity slider, and a context caret for slot-change,
  duplicate, and import.
- The active layer (highlighted) is where mask brushes write.
- Drag-to-reorder reorders the composite stack; the diffuse
  re-upload happens once on drop (not per-frame).
- Add layer is split into "from disk" (file picker) vs "empty
  layer in next-unused slot".

## Mask brushes

Four brushes drive the active layer's mask:

- **Reveal** — alpha → 1.0 (paint the layer in).
- **Hide** — alpha → 0.0 (paint it out).
- **Smooth** — blur mask values under the brush.
- **Fill** — set the entire footprint to the target value in one
  stamp.

Strength tunes per-stamp delta. Spacing tunes drag density (0.05
= dense overlap, 2.0 = sparse dots).

## What bakes into the .sd7

The build pipeline composites the stack into the diffuse BMP
that PyMapConv tiles + DXT1-compresses into the `.smt`, and
emits per-active-channel DNTS DDSes plus the splat distribution
PNG. The mapinfo `resources.splatDetailNormalTex` uses the
subtable form (PITFALL §15), not the legacy
`splatDetailNormalTex1..4` numbered keys.

A grey 1024² specular fallback is auto-baked when the project
doesn't specify a specular texture path. Without spec the map
renders flat (FINDINGS §7.2 — the gate moved off spec at the
C++ level, but the visual difference is still noticeable).
