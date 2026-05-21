# PITFALL §14 — Geo vents go through Springboard featureplacer, not a `geos = {}` array

Zero-K convention puts geo vents in a `geos = {…}` second array
inside `map_metal_layout.lua`. **BAR doesn't read that.** BAR's
`api_resource_spot_finder.GetSpotsGeo()` scans the map for
engine features with `FeatureDef.geoThermal = true` — typically
the stock `geovent` feature.

## Rule

`Project.geo_vents` emits ONLY into the **Springboard
featureplacer trio** with `name = "geovent"`. Never write a
`geos` array in `map_metal_layout.lua` — that file holds
`spots` only.

## The trio

```
LuaGaia/Gadgets/FP_featureplacer.lua   -- gadget (PD, vendored)
mapconfig/featureplacer/config.lua     -- one-liner redirect
mapconfig/featureplacer/set.lua        -- the data
```

`set.lua` returns:

```lua
local setcfg = {
  unitlist = {}, buildinglist = {},
  objectlist = {
    { name = "geovent", x = 4096, z = 4096, rot = 0 },
    -- … general features here too
  },
}
return setcfg
```

Plus the LuaGaia bootstrap pair from PITFALL §25 — without
those, the gadget never runs.

## What the pre-Sprint-11 editor did

The editor used to emit `mapconfig/featureplacer/features.lua`
— a path with **no consumer in BAR** (verified by grep across
the full Beyond-All-Reason checkout: zero matches in
`luarules/`, `luaui/`, `common/`). The file was silently
ignored; geo vents authored in the editor never spawned in-game.

Sprint 11 hotfix replaced the emission path with the Springboard
trio. The fix matches what `gecko_isle_remake_v1.2.1`,
`jade_empress_1.3`, `titanduel_v3`, … all ship.

## Diagnosis

If your geo vents don't spawn in BAR:

1. Unzip the `.sd7`. Verify `LuaGaia/Gadgets/FP_featureplacer.lua`
   is present.
2. Verify `LuaGaia/main.lua` is also present (PITFALL §25).
3. Verify `mapconfig/featureplacer/set.lua`'s `objectlist`
   includes `{ name = "geovent", … }` entries.
4. If all present and they still don't spawn, the gadget
   probably crashed at load — check BAR's `infolog.txt` for
   the gadget's error.
