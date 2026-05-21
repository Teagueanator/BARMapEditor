# Ally teams (reference)

Cross-tool reference for the per-side configuration model. The
**Start positions** tool article covers the in-place workflow;
this article documents the data model and emission rules.

## Model

```rust
struct AllyGroup {
    id: u8,                  // 0..15, palette-driven
    name: String,            // display label
    color: [u8; 4],          // RGBA, drives marker + lobby
    start_positions: Vec<StartPosition>,
    box_polygon: Option<Vec<[f32; 2]>>,  // elmo-space
    box_name_long: String,
    box_name_short: String,
}
```

The active group receives new placements. Switching active
groups doesn't touch the source list — placements add, deletions
remove. Symmetry only replicates source positions, not boxes
(box authoring is a Sprint 18+ surface).

## Emission

- `teams[i].startPos` — flat `{x, z}` per source (engine's
  required minimum; PITFALL §A).
- `mapconfig/map_startboxes.lua` — emitted only when ≥1 group
  has a `box_polygon`. Unwrapped per-ally-team table shape with
  elmo coordinates (PITFALL §26). When omitted, BAR's
  default-fallback codepath generates sensible N/S or E/W boxes.

## Default seeding

A fresh project starts with zero groups; the F8 tool adds a
default `AllyGroup` on first placement. The build pipeline emits
a 25 % / 75 % diagonal default pair when `ally_groups` is empty,
so even an unconfigured project produces a playable map.
