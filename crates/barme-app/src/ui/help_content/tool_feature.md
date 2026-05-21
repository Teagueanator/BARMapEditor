# Features

General feature placement — trees, rocks, props, wreckage. Geo
vents have their own tool because they're gameplay-critical and
deserve a distinct affordance; everything else lives here.

LMB places the picker-selected feature; LMB-drag on an existing
instance rotates it (`dx × 182` heading delta — roughly 1° per
pixel). RMB deletes. Symmetry replicates each source per mirror;
rotational symmetry rotates each copy's heading by
`65536 / fold` so per-sector visuals stay symmetric.

## Categories

The Inspector ships a category combo (`trees`, `rocks`, `props`,
`wreckage`, `geo`) and a filter field. Filter matches name,
display, or tag substrings, case-insensitive. The picker rows
show the feature's display name and a tag chip; click to arm.

## Stock feature catalogue

The current 30-entry baseline is sourced from
[beyond-all-reason/mapfeatures](https://github.com/beyond-all-reason/mapfeatures).
Auto-import from upstream is a polish task — for Sprint 22 the
catalogue is hand-curated at `assets/mapfeatures_catalog.json`.

## Rotation conventions

Rotation in the emitted `set.lua` is an **unquoted integer**
(PITFALL §23). The gadget calls
`Spring.CreateFeature(..., fDef.rot)` which expects a number;
PyMapConv's `-k` flat-text path uses quoted strings, but that's
a different codepath we don't use.
