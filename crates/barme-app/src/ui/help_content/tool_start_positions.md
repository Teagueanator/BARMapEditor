# Start positions

Per-ally-team spawn placement. Drives `teams[i].startPos` in
mapinfo (the engine's bare minimum — PITFALL §A) and feeds the
optional `map_startboxes.lua` when at least one ally has an
authored box polygon (PITFALL §26).

LMB places a position for the active ally team. LMB-drag across
empty terrain paints N positions distributed evenly along the
drag (configurable count in the Inspector). LMB on an existing
marker drag-moves it. RMB deletes. Symmetry replicates source
positions across the chosen axis.

## Ally teams

The Inspector ships a tree of ally teams. Each has a colour and a
display name. The colour drives both the canvas marker and the
in-game player colour assignment. The active team (star icon)
receives new placements.

## Presets

The Inspector offers stock layouts: `OneVOne`, `TwoVTwo`,
`EightVEight` (corner mirror), `ThreeWayFFA` (120° rotational),
`FourWayFFA` (quad). Applying a preset replaces the current
positions with the preset's source set; mirror/rotational
symmetry expands them into the final team-position grid.

## Start boxes

When ≥2 ally teams have an authored polygon, the build emits
`mapconfig/map_startboxes.lua` with elmo-space polygons (NOT
0..1 fractions — see PITFALL §26). When no polygon is authored
the file is omitted so BAR's default N/S or E/W fallback applies.
