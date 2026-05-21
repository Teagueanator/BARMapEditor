# Geo vents

Place steam vents — the `geovent` feature BAR scans for to seed
geothermal generator slots. Geo vents go into the Springboard
`set.lua`'s `objectlist` with `name = "geovent"` (PITFALL §21);
the `geos = {}` array from Zero-K convention is wrong for BAR
(PITFALL §14).

LMB places a vent at the cursor. LMB-drag moves; RMB deletes.
Symmetry replicates.

## What geo vents do in BAR

BAR's economy treats geo positions like metal spots — a per-vent
plume marker shows up under the F-key UI and your Construction
Bot or Geothermal-class building can lock to the spot. They are
the only renewable energy source on many competitive maps.

## Pipeline detail

The build pipeline stages the Springboard featureplacer trio at:

- `LuaGaia/Gadgets/FP_featureplacer.lua` (the gadget itself,
  vendored CC0)
- `mapconfig/featureplacer/config.lua` (one-liner redirect)
- `mapconfig/featureplacer/set.lua` (the data)

Plus the `LuaGaia/main.lua` + `draw.lua` bootstrap pair —
without them, BAR never scans `LuaGaia/Gadgets/` on map load
(PITFALL §25).
