# PITFALL §18 — `lighting.sunDir.w = 1.0`

The W component of `lighting.sunDir` is an intensity scalar.
Engine default is `float4(0.0, 1.0, 2.0, 1.0)` — W = `1.0`.
Earlier research's `1e9` default leaked from a different code
path (`sunStartDistance`); emitting it over-saturates sunlight
on map load and the terrain reads pure white.

## Rule

`MapInfo::bar_default()`'s `lighting.sun_dir` uses
`[0.5, 0.7, 0.5, 1.0]` (any normalised direction + W = 1.0). A
unit test pins W to exactly 1.0.

## Diagnosis

If your map renders correctly in the editor but is washed out
in BAR:

1. Open the emitted mapinfo.lua.
2. Find `lighting.sunDir = { x, y, z, w }`.
3. The fourth component should be ≈ 1.0. If it's `1e9`,
   `1000000000`, or anything outside `[0, 2]`, the value
   is wrong.
4. The editor regenerates this on every save — re-saving the
   project fixes it.

## Why W is intensity

Spring's lighting model uses the direction as a 3D unit vector
and the W component as a brightness multiplier. Sprint 10's
mapinfo audit pinned this; the change was load-bearing for the
first live-BAR smoke test (start positions appeared missing
because BAR refused to load the broken sunDir).
