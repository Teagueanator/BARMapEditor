# BAR mapinfo.lua + Gadget Schema Reference

**TL;DR**
- BAR maps boot from a single root-level `mapinfo.lua` parsed by `rts/Map/MapInfo.cpp` in the Recoil engine (Recoil is a continuation/extension of Spring RTS engine version 105.0, per the official RecoilEngine wiki; the fork point is visible in git tags such as `spring_bar_{BAR105}105.0-430-g2727993`). The file is a deep table with 18 top-level scalar keys and 12 named sub-tables. The emitter currently fills <10% of the consumable surface — the rest of this document enumerates the remaining ~90 fields and which engine/gadget code reads each.
- Metal spots and geo vents are NOT primarily a `mapinfo.*` concern; BAR's canonical mechanism is a Lua spots table at `mapconfig/map_metal_layout.lua` consumed by the `resource_spot_finder` gadget which exposes `GG['resource_spot_finder'].metalSpotsList / .geoSpotsList`. Features live at `mapconfig/featureplacer/features.lua` (or a sibling `set.lua`). The emitter should bundle Lua sidecars, not bake into the SMF metalmap.
- The featureless/untextured boot symptom is caused by missing `resources.*` texture references, missing `splats.*` and missing `atmosphere`/`lighting` blocks rather than missing `smf` fields. The minimum useful expansion is `resources`, full `lighting`, full `atmosphere`, `splats`, `water`, and the spots/features sidecars.

## 1. Full mapinfo.lua field table

Coordinates: `x`/`z` are in elmos (engine map units, 1 elmo = 1 world unit). Map size in elmos is `Game.mapSizeX = mapInfo.smf.mapx * SQUARE_SIZE * 64` style — see Recoil `rts/Sim/Misc/GlobalConstants.h`.

| Path | Type | Default | Consumed by | Required for BAR? | Description |
|---|---|---|---|---|---|
| `name` | string | "" | `rts/Map/MapInfo.cpp` `ReadGlobal`; BYAR-Chobby map list | yes | Display name. |
| `shortname` | string | "" | `MapInfo.cpp` `ReadGlobal` | no | Short tag. |
| `description` | string | "" | `MapInfo.cpp` `ReadGlobal` | no | Tooltip. |
| `author` | string | "" | `MapInfo.cpp` `ReadGlobal` | no | Author credit. |
| `version` | string | "1.0" | engine archive index | yes (Chobby) | Map version; appears in archive name. |
| `mapfile` | string | "" | `MapInfo.cpp` `ReadGlobal` → `SMFReadMap` | yes | Path to `.smf` inside the `.sd7`, e.g. `"maps/quicksilver.smf"`. |
| `modtype` | int | 0 | engine archive scanner; **BYAR-Chobby map filter** | **yes (3)** | Must equal 3 or the map is invisible to BAR's lobby. |
| `depend` | string[] | {} | engine archive resolver | **yes** | Must contain `"Map Helper v1"`. Add `"Spring Bitmaps"` if using shared textures. |
| `replace` | string[] | {} | engine archive resolver | no | Archives this map supersedes. |
| `maphardness` | float | 100 | `MapInfo.cpp` `ReadGlobal`; `Sim/Map/Ground.cpp` deformation | no | Resistance to crater deformation. Higher = stiffer. |
| `notDeformable` | bool | false | `MapInfo.cpp` `ReadGlobal` | no | Disables terrain deformation entirely. |
| `gravity` | float | 130 | `MapInfo.cpp` `ReadGlobal`; `Sim/Misc/Wind.cpp` | no | Per-frame gravity; BAR convention is 130. |
| `tidalStrength` | float | 0 | `MapInfo.cpp` `ReadGlobal`; tidal generator unit | conditional | Non-zero on water maps for Tidal Generators. |
| `maxMetal` | float | 0.02 | `MapInfo.cpp` `ReadGlobal`; engine metalmap loader | yes (water/raw mex maps) | Per-pixel scale of the metalmap.png; BAR uses Lua spots so this is mostly ignored. |
| `extractorRadius` | float | 500 | `MapInfo.cpp` `ReadGlobal`; `Sim/Units/UnitDef.cpp` | yes | Engine-wide exclusion radius. BAR convention: **80**. |
| `voidWater` | bool | false | `MapInfo.cpp`; `Rendering/Env/WaterRendering.cpp` | no | Removes water plane (Apophis style); requires omitting `water.planeColor`. |
| `voidGround` | bool | false | `MapInfo.cpp` `ReadGlobal` | no | Alpha-cuts ground using diffuse alpha channel. |
| `autoShowMetal` | bool | true | `MapInfo.cpp`; `Game/UI/InfoTexture` | no | Auto F4 view on mex queue. |
| `mapHardness` (alias) | float | — | same | no | Older lowercase variant; both keys are accepted. |
| **`smf.*`** | table | — | `Map/SMF/SMFReadMap.cpp` | yes (some) | See section 6 for full list. |
| **`atmosphere.*`** | table | — | `Rendering/Env/SkyLight.cpp`, `Wind.cpp` | yes | Wind range, fog, sky color, skybox. |
| **`lighting.*`** | table | — | `Rendering/Env/SunLighting.cpp` | **yes** | Sun direction, ground/unit ambient/diffuse/specular. |
| **`water.*`** | table | — | `Rendering/Env/WaterRendering.cpp` | conditional | Required iff `tidalStrength > 0` or terrain dips below 0. |
| **`splats.*`** | table | — | `Map/SMF/SMFGroundDrawer.cpp` | yes (DNTS maps) | RGBA splat scale/mult arrays. |
| **`grass.*`** | table | — | `Rendering/Env/GrassDrawer.cpp` | no | Grass blade params; BAR mostly uses `map_grass_gl4.lua` widget instead. |
| **`teams[i].startPos.{x,z}`** | int | — | `Map/MapInfo.cpp` `ReadTeams`; `Game/GameSetup.cpp` `LoadStartPositions` | **yes** | Default start positions in elmos. `allyTeam` is **not** here — it lives in `script.txt`. |
| **`terrainTypes[i].*`** | table | — | `MapInfo.cpp` `ReadTerrainTypes`; `MoveDefHandler.cpp` | conditional | Per-typemap-index movement modifiers, hardness, tracks. |
| **`resources.*`** | table | — | `MapInfo.cpp` `ReadResources`; `Rendering/Map/*` | yes | Specular/detail/splat/grass/normal/PBR texture filenames. |
| **`custom.*`** | table | — | `MapInfo.cpp` `ReadCustom`; surfaced as `Spring.GetMapOptions()` / `Game.mapOptions` | no | Free-form mapper data; consumed by gadgets/widgets. |
| **`sound.*`** | table | — | `Sound/EFXPresets.cpp` | no | Reverb preset + override params. |
| **`gui.*`** | table | — | engine UI | no | Minimap rotation hint. |

Top-level legacy aliases also accepted by `MapInfo.cpp`: `startpic`, `StartMusic` (both deprecated; ignored), `mutator` (ignored).

## 2. allyTeam / teams[] schema

**Confirmed:** `mapinfo.teams[i]` carries **only** `startPos = {x, z}`. `allyTeam` is **not** a `mapinfo.lua` field — it is supplied per match by the lobby in `script.txt`'s `[TEAM_i]` block (`AllyTeam = N`), and read by `rts/Game/GameSetup.cpp` `CGameSetup::Init`. BAR's autohosts (SPADS) and Chobby also push lobby-level `mapBoxes` from `beyond-all-reason/maps-metadata` (`gen/mapBoxes.conf` per the Makefile) which overrides per-game.

**Implication for the emitter:** ordered numeric indices `[0..N-1]` are simply a *pool* of geographic positions. The engine's lobby/start-position logic picks from this pool by team-id at game-start; the editor does not need to encode allyteam membership inside `mapinfo.teams`. What the editor SHOULD emit is one entry per geographic spawn the map supports, and (in a sibling file) `mapconfig/map_startboxes.lua` declaring rectangles by ally count.

### Canonical layouts

**1v1** (2 team slots, 1 per ally):
```lua
teams = {
  [0] = {startPos = {x = 1024, z =  768}},   -- north
  [1] = {startPos = {x = 7168, z = 7424}},   -- south
},
```

**8v8** (16 spawns, 8 per ally). Real BAR practice on Quicksilver-class maps is to emit all 16 positions and let the lobby's startbox script + player count slice them. Example pattern (axially symmetric):
```lua
teams = {
  -- ally 0 (north strip)
  [0] = {startPos = {x =  640, z =  640}},
  [1] = {startPos = {x = 1920, z =  640}},
  [2] = {startPos = {x = 3200, z =  640}},
  [3] = {startPos = {x = 4480, z =  640}},
  [4] = {startPos = {x = 5760, z =  640}},
  [5] = {startPos = {x = 7040, z =  640}},
  [6] = {startPos = {x = 8320, z =  640}},
  [7] = {startPos = {x = 9600, z =  640}},
  -- ally 1 (south strip)
  [8]  = {startPos = {x =  640, z = 9344}},
  [9]  = {startPos = {x = 1920, z = 9344}},
  [10] = {startPos = {x = 3200, z = 9344}},
  [11] = {startPos = {x = 4480, z = 9344}},
  [12] = {startPos = {x = 5760, z = 9344}},
  [13] = {startPos = {x = 7040, z = 9344}},
  [14] = {startPos = {x = 8320, z = 9344}},
  [15] = {startPos = {x = 9600, z = 9344}},
},
```

**3-way FFA** (e.g. Comet Catcher style, 3 allies × N teams):
```lua
teams = {
  [0] = {startPos = {x = 4000, z =  900}},   -- ally 0 north
  [1] = {startPos = {x =  900, z = 7200}},   -- ally 1 SW
  [2] = {startPos = {x = 7200, z = 7200}},   -- ally 2 SE
},
```
The accompanying `mapconfig/map_startboxes.lua` declares the three triangular start boxes; ally membership of slots is decided there + script.txt.

### Editor emitter rule (recommended)

The Rust model should be:
```rust
struct StartPos { x: i32, z: i32 }
struct MapStartConfig {
    /// Flat ordered pool — every concrete spawn coordinate.
    spawns: Vec<StartPos>,
    /// Logical groupings for startbox generation, separate from `teams[]`.
    ally_groups: Vec<AllyGroup>,
}
struct AllyGroup { name: String, polygon: Vec<(f32,f32)>, default_spawns: Vec<usize> }
```
Emit `spawns` into `mapinfo.teams[i]` and emit `ally_groups` into `mapconfig/map_startboxes.lua` (see section 5 file layout). The editor must *not* encode `allyTeam` in `mapinfo.lua` — it will be silently ignored by the engine and confuse downstream tools.

## 3. Metal spots — F5 implementation guidance

**Recommendation: emit a Lua spots file at `mapconfig/map_metal_layout.lua`, ship a blank metalmap.png compiled into the SMF (or omit the metalmap entirely), and do NOT use the metalmap-baked-into-SMF model.**

### Why

BAR's `resource_spot_finder` gadget (the consumer of `GG['resource_spot_finder'].metalSpotsList`) descends directly from Zero-K's `mex_spot_finder.lua` at `ZeroK-RTS/Zero-K/blob/master/LuaRules/Gadgets/mex_spot_finder.lua` and inherits its identical config constants (`MAPSIDE_METALMAP = "mapconfig/map_metal_layout.lua"`, `ALT_MAPSIDE_METALMAP = "mapconfig/map_resource_spot_layout.lua"`, `GAMESIDE_METALMAP = "LuaRules/Configs/MetalSpots/"`). It reads sources in this priority order:

1. `mapconfig/map_metal_layout.lua` (map-side, primary)
2. `mapconfig/map_resource_spot_layout.lua` (legacy alias)
3. `LuaRules/Configs/MetalSpots/<mapname>.lua` (game-side override; rarely used in BAR)
4. Engine metalmap.png — fallback only, produces poorly-clustered blob spots

Every modern BAR widget that draws mex halos, snap-to-spot, mex placement AI, and the BAR ruins gadget at `luarules/gadgets/ai_ruins.lua` lines 223–253 (per DeepWiki's index of beyond-all-reason/Beyond-All-Reason: *"The getNearestBlocker(x, z) function ensures ruins do not block critical map features by checking distance to metalSpots and geoSpots via the resource_spot_finder GG API"*) reads from this gadget's table, not from the raw `Spring.GetMetalAmount` API. Maps that ship a metalmap-only setup get suboptimal spot clustering and are visibly worse to play.

### Exact Lua format

```lua
-- mapconfig/map_metal_layout.lua
return {
  spots = {
    { x = 1024, z =  768, metal = 2.0 },   -- coordinates in elmos
    { x = 7168, z = 7424, metal = 2.0 },
    { x = 4096, z = 4096, metal = 4.0 },   -- center, double yield
    -- ...
  },
  -- Optional. If set without spots, all engine-metalmap spots get this value.
  -- If set with spots, used as fallback for spots without explicit `metal`.
  metalValueOverride = 2.0,
}
```

Required fields per spot: `x`, `z`. Optional: `metal` (production multiplier; default falls back to engine metalmap sample or 2.0). Coordinates are in **elmos** (world units), not metalmap pixels.

### Consumers (gadgets that REQUIRE this format)

- `WG['resource_spot_finder'].metalSpotsList` — consumed by `luaui/Widgets/map_grass_gl4.lua` (clears grass around spots), the mex placement/snap widgets, and the F4 mex view.
- `GG['resource_spot_finder']` — consumed by `luarules/gadgets/ai_ruins.lua` lines 223–253 (avoids placing ruins on spots) and Shard AI's `MetalSpotHandler` (`luarules/gadgets/ai/Shard/spothandler.lua`).
- AI gamerulesparams: `mex_count`, `mex_x#`, `mex_z#`, `mex_metal#` are published from the same source.

### Rust model

```rust
struct MetalSpot { x: i32, z: i32, value: f32 }
struct MapResources {
    metal_spots: Vec<MetalSpot>,
    metal_value_override: Option<f32>,
    geo_spots: Vec<GeoVent>,
}
```
Do **not** add a `metal_map: PathBuf` to the public model. The SMF compiler (PyMapConv) needs *some* metalmap PNG to bake into the SMF, but ship a fully-black 1×1 PNG so the engine fallback finds nothing — the Lua spots are the source of truth.

## 4. Geo vents — F6 implementation guidance

**Geo vents are a SECOND ARRAY in the same `map_metal_layout.lua` file**, not a `mapinfo.*` block, not a separate gadget input. The `resource_spot_finder` gadget builds `geoSpotsList` from any `geos` table in the same map-side config, plus by scanning the map for engine features with `geoThermal = true` (the canonical stock feature is `geovent` from BAR's mapfeatures repo).

### Recommended layout

```lua
-- mapconfig/map_metal_layout.lua  (same file as metal spots)
return {
  spots = { ... metal spots ... },
  geos  = {
    { x = 2048, z = 4096 },
    { x = 8192, z = 4096 },
  },
}
```

Plus emit one engine-feature placement per geo in the feature placer (see section 5) using stock feature name `geovent` so it renders as a steam vent visually:
```lua
-- mapconfig/featureplacer/features.lua
{ name = "geovent", x = 2048, z = 4096, rot = "0" },
```

### Consumers

- `WG['resource_spot_finder'].geoSpotsList` — same spot schema as metal: `{x, z, minX, maxX, minZ, maxZ}`. Read by `luaui/Widgets/map_grass_gl4.lua` (grass clearance) and the F4 mex/geo view.
- `luarules/gadgets/ai_ruins.lua` lines 223–253 — uses geo spots to avoid blocking ruins on them.

### Rust model

```rust
struct GeoVent { x: i32, z: i32 }
```
Emit BOTH a `geos` entry in `map_metal_layout.lua` AND a `geovent` feature in `featureplacer/features.lua` for each geo. Skipping the feature gives no visible steam plume.

## 5. Features — F7 implementation guidance

### Canonical file location

`mapconfig/featureplacer/features.lua` — this is the Recoil/Spring-engine convention used by Springboard's export and by BAR's `mapfeatures` repo (`github.com/beyond-all-reason/mapfeatures`). Some older BAR maps put the same content at `LuaGaia/Gadgets/featureplacer.lua` or use a sibling `set.lua` invoked by an `FP_featureplacer.lua` gadget — both still work, but the `mapconfig/featureplacer/` path is what current BAR mapping documentation specifies.

### Per-feature schema

```lua
return {
  { name = "btree_lg_a", x = 1234, z = 5678, rot = "0" },
  { name = "rock_a",     x = 2000, z = 3000, rot = "16384" },  -- 16384 = 90° heading
  { name = "geovent",    x = 2048, z = 4096, rot = "0" },
}
```

- `name` — string, FeatureDef name (must resolve to a FeatureDef in the BAR mod or in a bundled `features/` dir).
- `x`, `z` — elmos, integer. **`y` is intentionally omitted** — the engine samples ground height at spawn, which means features re-float correctly on water-level changes (the official BAR mapping guide explicitly warns: *"Ensure features do not use a fixed Y value, so they will float in the air on water-height changes"*). Override only for floating wreckage.
- `rot` — string-typed integer in **Spring heading units** (`"0"` through `"65535"` represents 0–360°). Single Y-axis rotation only; no full Euler / matrix support. The legacy SMD `rot = "0"` string convention persists; the parser also accepts numbers. **Use string form for compatibility.**
- `scale` — **not supported by the standard feature placer**. To get a different-sized rock, ship a separate FeatureDef. Some BAR maps add a `customparams = { altmodel = "..." }` per feature but this is not engine-standard.
- No allyteam ownership on map features — they belong to the Gaia team automatically.

### "Stock" features in BAR (bundled in `github.com/beyond-all-reason/mapfeatures`)

The `mapfeatures` companion archive supplies the names mappers reference for zero `.sd7` payload cost. Naming patterns:

- **Pine trees:** `allpinesb_ad0_<color><variant><size>` where color ∈ {green, brown, snowgreen, snow}, variant ∈ {a,b,c}, size ∈ {xs, s, m, l, xl, xxl}. Example: `allpinesb_ad0_greena_m`.
- **Geo vents:** `geovent` (single canonical name; renders as steam-emitting vent).
- **Rocks and other vegetation:** follow the same `<set>_<variant>` convention inside `mapfeatures`.
- **Unit wrecks:** generated by `gamedata/featuredefs_post.lua` from unit defs as `<unitname>_dead` and `<unitname>_heap` (e.g. `armcom_dead`, `corcom_heap`). Mappers can place these for atmospheric wreckage.

### Bundling mechanism for map-custom features

If a mapper needs a feature not in `mapfeatures`, the `.sd7` must contain:

- `features/<myfeature>.lua` — full `FeatureDefs` Lua table (mirrors BAR's unitdef-post structure).
- `objects3d/<myfeature>.s3o` — model (`.s3o` is canonical; `.dae`/`.3do` legacy-supported; `.obj` is **not** accepted by the engine).
- `unittextures/<myfeature>_diffuse.dds` and `<myfeature>_other.dds` (S3O atlas pair).

Add `"Spring Bitmaps"` to `mapinfo.depend` if the feature reuses any bundled textures. The SRS's existing claim is **correct**: BAR-mod stock features are zero `.sd7` payload but limit you to the mapfeatures library; bundling adds your model + textures into the above paths.

### Recommended emitter behavior

- Emit a flat `Vec<FeatureInstance>` to `mapconfig/featureplacer/features.lua`.
- Default the editor's feature picker to the `mapfeatures` repo's vegetation list (lazy-load names from a JSON manifest the editor team can scrape once from that repo).
- For map-custom features, the editor must own the bundling step (copy `.s3o`/`.dds`/`.lua` triplet into the archive's `features/`, `objects3d/`, `unittextures/`).

## 6. Other mapinfo blocks — F9 schema

### atmosphere
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `atmosphere.minWind` | float | 5.0 | `Sim/Misc/Wind.cpp` | yes | Min wind energy; clamped to maxWind. |
| `atmosphere.maxWind` | float | 25.0 | `Sim/Misc/Wind.cpp` | yes | Max wind energy. BAR convention: 5–25 for balanced maps. |
| `atmosphere.fogStart` | float | 0.1 | `Rendering/Env/MapRendering.cpp` | no | Fog start as fraction of view range. **Setting to 1.0 breaks build ETA rendering** (known landmine, see §7). |
| `atmosphere.fogEnd` | float | 1.0 | same | no | Fog opacity end fraction. |
| `atmosphere.fogColor` | rgb | {0.7,0.7,0.8} | same | no | Below-world infinite-plane color too. |
| `atmosphere.sunColor` | rgb | {1,1,1} | `Rendering/Env/SkyLight.cpp` | no | Sun disc tint AND size (values >1 grow the disc). |
| `atmosphere.skyColor` | rgb | {0.1,0.15,0.7} | same | no | Sky tint. |
| `atmosphere.skyDir` | vec3 | {0,0,-1} | same | no | Sky orientation; rarely changed. |
| `atmosphere.skyBox` | string | "" | `Rendering/Env/ISky.cpp` | no | Skybox DDS cube filename (in `maps/` or `bitmaps/`). |
| `atmosphere.cloudDensity` | float | 0.5 | same | no | 0..1 cloud cover. |
| `atmosphere.cloudColor` | rgb | {1,1,1} | same | no | Cloud tint. |

Why touch: skyboxes change map identity, and `minWind/maxWind` directly tunes the economy.

### water
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `water.damage` | float | 0 | `Sim/Map/Water.cpp` | no | DPS to non-amphibious units in water. |
| `water.repeatX` | float | 0 | `WaterRendering.cpp` | no | Water texture tile X. |
| `water.repeatY` | float | 0 | same | no | Water texture tile Y. |
| `water.surfaceColor` | rgb | {0.75,0.8,0.85} | same | yes (water maps) | Above-surface tint. |
| `water.planeColor` | rgb | unset | same | conditional | **Must be omitted for `voidWater = true` to work.** |
| `water.absorb` | rgb | {0,0,0} | same | no | Per-channel light absorption. |
| `water.baseColor` | rgb | {0.4,0.7,0.8} | same | no | Underwater base color. |
| `water.minColor` | rgb | {0.1,0.2,0.3} | same | no | Min underwater color clamp (read at deep z). |
| `water.ambientFactor` | float | 1.0 | same | no | Underwater ambient multiplier. |
| `water.diffuseFactor` | float | 1.0 | same | no | Underwater diffuse multiplier. |
| `water.specularFactor` | float | 1.0 | same | no | Specular multiplier. |
| `water.specularColor` | rgb | sunColor | same | no | Specular tint. |
| `water.specularPower` | float | 20 | same | no | Specular exponent. |
| `water.fresnelMin` | float | 0.2 | same | no | Reflection floor. |
| `water.fresnelMax` | float | 0.8 | same | no | Reflection ceiling. |
| `water.fresnelPower` | float | 4 | same | no | Fresnel exponent. |
| `water.reflectionDistortion` | float | 1 | same | no | Wave distortion strength. |
| `water.blurBase` | float | 2 | same | no | Reflection blur radius. |
| `water.blurExponent` | float | 1.5 | same | no | Blur falloff. |
| `water.perlinStartFreq` | float | 8 | same | no | Animated normal frequency. |
| `water.perlinLacunarity` | float | 3 | same | no | Octave lacunarity. |
| `water.perlinAmplitude` | float | 0.9 | same | no | Octave amplitude. |
| `water.numTiles` | int | 1 | same | no | Tile count. |
| `water.shoreWaves` | bool | true | same | no | Render foam at shore. |
| `water.forceRendering` | bool | false | same | no | Force water draw even with no underwater terrain. |
| `water.texture` | string | "" | same | no | Diffuse override. |
| `water.foamTexture` | string | "" | same | no | Foam diffuse. |
| `water.normalTexture` | string | "" | same | no | Normal map. |
| `water.caustics` | string[] | {} | same | no | Animated caustic texture frames. |

Why touch: tidal/water-level maps need at least `surfaceColor`, `planeColor`, `minColor` set; everything else uses BAR-tuned defaults.

### lighting
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `lighting.sunDir` | vec3+w | {0,1,2,1e9} | `Rendering/Env/SunLighting.cpp`; widget `widget:SunChanged` callin | **yes** | Sun direction. Fourth component is sunStart distance. **See §7 — many widgets assume non-nil.** |
| `lighting.groundAmbientColor` | rgb | {0.5,0.5,0.5} | `SunLighting.cpp` | yes | Terrain ambient. |
| `lighting.groundDiffuseColor` | rgb | {0.5,0.5,0.5} | same | yes | Terrain diffuse. |
| `lighting.groundSpecularColor` | rgb | {0.1,0.1,0.1} | same | no | Terrain specular tint. |
| `lighting.groundShadowDensity` | float | 0.8 | same | no | 0..1 shadow strength on ground. |
| `lighting.unitAmbientColor` | rgb | {0.4,0.4,0.4} | same | yes | Unit ambient. |
| `lighting.unitDiffuseColor` | rgb | {0.7,0.7,0.7} | same | yes | Unit diffuse. |
| `lighting.unitSpecularColor` | rgb | unitDiffuseColor | same | no | Unit specular. |
| `lighting.unitShadowDensity` | float | 0.8 | same | no | Shadow strength on units. |
| `lighting.specularExponent` | float | 100 | same | no | Specular sharpness. |

Why touch: this is the single biggest visual control surface; the default {0.5} grays look flat.

### terrainTypes
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `terrainTypes[i].name` | string | "Default" | `MapInfo.cpp` `ReadTerrainTypes` | no | Display name. |
| `terrainTypes[i].hardness` | float | 1.0 | same | no | Multiplier × `maphardness`. |
| `terrainTypes[i].receiveTracks` | bool | true | `Rendering/Env/GroundDecals` | no | Vehicle tracks visible. |
| `terrainTypes[i].moveSpeeds.tank` | float | 1.0 | `Sim/MoveTypes/MoveDefHandler.cpp` | no | Tank movement scalar. |
| `terrainTypes[i].moveSpeeds.kbot` | float | 1.0 | same | no | Kbot scalar. |
| `terrainTypes[i].moveSpeeds.hover` | float | 1.0 | same | no | Hover scalar. |
| `terrainTypes[i].moveSpeeds.ship` | float | 1.0 | same | no | Ship scalar. |

Indices 0..255 map to per-pixel byte values in the typemap. Most BAR maps use 0..3.

### splats
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `splats.texScales` | float[4] | {0.02,0.02,0.02,0.02} | `SMFGroundDrawer.cpp` | yes (splat-textured maps) | UV scale per RGBA channel. |
| `splats.texMults` | float[4] | {1,1,1,1} | same | yes | Intensity per RGBA channel. |

### resources
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `resources.detailTex` | string | "" | `SMFGroundDrawer.cpp` | yes | Detail tile texture filename. |
| `resources.specularTex` | string | "" | same | yes (SSMF) | Per-pixel specular map. |
| `resources.splatDetailTex` | string | "" | same | yes (splats) | Splat-channel detail texture. |
| `resources.splatDistrTex` | string | "" | same | yes (splats) | RGBA distribution map. |
| `resources.splatDetailNormalTex` | string[] | {} | same | conditional (DNTS) | Up to 4 detail normal textures + `alpha = true`. |
| `resources.splatDetailNormalDiffuseAlpha` | int | 0 | same | conditional | Mix diffuse into normal alpha. |
| `resources.skyReflectModTex` | string | "" | same | no | Sky reflection modulation. |
| `resources.detailNormalTex` | string | "" | same | no | Global detail normal map. |
| `resources.lightEmissionTex` | string | "" | same | no | Emissive map. |
| `resources.parallaxHeightTex` | string | "" | same | no | Parallax height. |
| `resources.grassBladeTex` | string | "" | `GrassDrawer.cpp` | no | Grass blade texture (BAR usually overrides via widget). |
| `resources.grassShadingTex` | string | minimap | same | no | Grass color modulation. |

### custom
Free-form table surfaced as `Spring.GetMapOptions()` plus directly readable from `mapinfo.custom` in gadgets. Published BAR maps use it for:
- `custom.fog = { ... }` — dual_fog gadget parameters (`gui_dualfog_gadget.lua`).
- `custom.precipitation = { ... }` — rain/snow gadget (`precipitation.lua`).
- `custom.clouds = { speed=, color=, ... }` — `spring_volfog`-style volumetric fog widget.

The emitter should expose `custom` as `HashMap<String, serde_json::Value>` to allow round-tripping any future gadget config without schema bumps.

### smf extras
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `smf.minHeight` | float | from .smf | `SMFReadMap.cpp` | yes | Override compiled min height. Use **camelCase** — engine accepts both but BAR convention is `minheight` (lowercase also works). |
| `smf.maxHeight` | float | from .smf | same | yes | Override compiled max height. |
| `smf.minimapTex` | string | "" | same | no | Override minimap .dds. |
| `smf.smtFileName0` | string | from .smf | same | yes (on rename) | **Must match the actual `.smt` file in the `.sd7`** — pink-map landmine, see §7. |
| `smf.smtFileName1..N` | string | "" | same | no | Multi-SMT maps (rare in BAR). |

Engine-supported but BAR-unused (safe to skip): `smfheight`, `featuresFile`, `metalmap`, `typemap`, `vegetationmap`, `customMetaTex`, `blendNormalsTex`. These exist in the engine struct but are zero-referenced by current BAR gadgets; PyMapConv bakes most into the SMF binary instead.

### gui
| Path | Type | Default | Consumed by | Required? | Description |
|---|---|---|---|---|---|
| `gui.minimapRotation` | int | 0 | minimap renderer | no | Degrees. Almost never used.

## 7. Silent-failure landmines

- **`lighting.sunDir`** (note: engine key is `sunDir`, not `sundir`) — read by `Rendering/Env/SunLighting.cpp` and by widgets implementing `widget:SunChanged`. The "unit_sunfacing.lua crashes on nil" claim from the editor team's brief **could not be verified in current BAR master** — no file literally named `unit_sunfacing.lua` is present. The closest match is `luarules/gadgets/map_nightmode.lua` which calls `Spring.SetSunDirection` and assumes a valid baseline. Treat `lighting.sunDir` as mandatory regardless — the engine defaults are visibly wrong on most maps.
- **`modtype = 3`** — required for the map to appear in BYAR-Chobby's map browser; the lobby filters by `modtype == 3` when populating the map list. If omitted (defaults to 0), the archive scans as a hidden/base content type and the map is invisible.
- **`smf.smtFileName0`** — must match the actual SMT filename inside the `.sd7`. The SMT name is hardcoded in the binary SMF header; if the SMT is renamed in the archive and `smtFileName0` doesn't override, the engine cannot resolve the texture pages and the map renders entirely pink. This is the canonical "pink map on rename" pitfall called out in `springrts.com/wiki/Mapdev:Main`.
- **`depend = {"Map Helper v1"}`** — without this dependency, the engine cannot resolve standard map base content (default detail textures, default grass shader); the map either fails to load or renders with engine fallbacks (visible as the "untextured" symptom the editor team is hitting).
- **`atmosphere.fogStart` / `atmosphere.fogEnd`** — setting both to 1.0 (a tempting "no fog" attempt) has been reported to break build ETA / health-bar rendering on the affected map. The specific source thread cited in earlier drafts could not be confirmed at a particular springrts.com URL during research, but the failure pattern is reproducible per multiple community accounts. Use `fogStart = 0.99, fogEnd = 1.0` to disable fog safely.
- **`water.planeColor`** — must be **absent** (not set to `{0,0,0,0}`) for `voidWater = true` to actually show the skybox below the world. Setting it to any value defeats voidWater silently.
- **`tidalStrength`** without `water.*` block — Tidal Generator units pull energy from `tidalStrength`, but the water plane stays at default colors and looks broken if no `water.surfaceColor` is set. The emitter should enforce: `if tidalStrength > 0 then require water block`.
- **`extractorRadius`** mismatch with `mapconfig/map_metal_layout.lua` spots — the engine uses `extractorRadius` to compute the exclusion zone for mex placement, and `resource_spot_finder` uses it to compute `spot.minX/maxX/minZ/maxZ` bounds. BAR convention is **80**; using the default 500 produces giant overlapping spot bounds and breaks mex snap.
- **`features` referenced by name not present in `Spring Features`/`mapfeatures` and not bundled** — the engine logs `Error: [GetFeatureDef] could not find FeatureDef "artmapleoldlo"` per a real BAR error log, and the feature silently doesn't spawn. Editor must validate every feature name against the mapfeatures manifest at emit time.
- **`custom = { fog = ... }` missing when `gui_dualfog_gadget.lua` is included** — the gadget fails to load with `"Can't find settings in mapinfo.lua!"` per `springrts.com/phpbb/viewtopic.php?t=33209`. If the editor enables dual-fog, it MUST populate `custom.fog`.
- **`mapconfig/map_startboxes.lua` missing** — without this sidecar, SPADS autohosts default to whole-map startboxes for every game, producing terrible matchups. Not a crash, but a "ranked-play unfit" silent failure. Editor must emit it whenever ≥ 1v1.
- **`teams[]` count < lobby slot count** — the engine wraps to `teams[0]` for overflow slots, causing all extra players to spawn stacked at the first position. Emit at least 16 spawns for any BAR map intended for big-team queue.

## 8. Recommended emitter output (Phase 4 / Phase 5)

Below is the Phase-5 target. Drop into a hypothetical 8v8 land+water 10240×10240 elmo map.

```lua
-- mapinfo.lua (root of .sd7)
local mapinfo = {
  name        = "Example Plains 1.0",
  shortname   = "ExamplePlains",
  description = "10k x 10k twin-spawn-strip 8v8 with central water gap",
  author      = "BAR Editor v0.5",
  version     = "1.0",
  mapfile     = "maps/example_plains.smf",
  modtype     = 3,
  depend      = { "Map Helper v1", "Spring Bitmaps" },
  replace     = {},

  maphardness     = 100,
  notDeformable   = false,
  gravity         = 130,
  tidalStrength   = 18,
  maxMetal        = 0.02,
  extractorRadius = 80.0,
  voidWater       = false,
  voidGround      = false,
  autoShowMetal   = true,

  smf = {
    minHeight    = -120,
    maxHeight    =  640,
    smtFileName0 = "maps/example_plains.smt",
    minimapTex   = "",
  },

  resources = {
    detailTex             = "detail_grass.dds",
    specularTex           = "spec.dds",
    splatDetailTex        = "splat_detail.dds",
    splatDistrTex         = "splat_distr.dds",
    splatDetailNormalTex  = {
      "rock_dnts.dds", "grass_dnts.dds", "sand_dnts.dds", "dirt_dnts.dds",
      alpha = true,
    },
    splatDetailNormalDiffuseAlpha = 1,
    skyReflectModTex      = "",
    detailNormalTex       = "",
    lightEmissionTex      = "",
    parallaxHeightTex     = "",
    grassBladeTex         = "",
    grassShadingTex       = "",
  },

  splats = {
    texScales = { 0.008, 0.008, 0.012, 0.012 },
    texMults  = { 1.0,   1.0,   0.8,   0.8   },
  },

  atmosphere = {
    minWind      = 5.0,
    maxWind      = 22.0,
    fogStart     = 0.45,
    fogEnd       = 0.99,
    fogColor     = { 0.78, 0.82, 0.88 },
    sunColor     = { 1.00, 0.97, 0.92 },
    skyColor     = { 0.55, 0.70, 0.95 },
    skyDir       = { 0.0, 0.0, -1.0 },
    skyBox       = "skyboxes/clear_day.dds",
    cloudDensity = 0.30,
    cloudColor   = { 1.00, 1.00, 1.00 },
  },

  lighting = {
    sunDir               = { 0.3, 1.0, -0.2, 1.0e9 },
    groundAmbientColor   = { 0.50, 0.50, 0.55 },
    groundDiffuseColor   = { 0.80, 0.78, 0.72 },
    groundSpecularColor  = { 0.10, 0.10, 0.10 },
    groundShadowDensity  = 0.75,
    unitAmbientColor     = { 0.45, 0.45, 0.50 },
    unitDiffuseColor     = { 0.85, 0.82, 0.78 },
    unitSpecularColor    = { 0.85, 0.82, 0.78 },
    unitShadowDensity    = 0.75,
    specularExponent     = 100,
  },

  water = {
    damage             = 0,
    surfaceColor       = { 0.40, 0.55, 0.68 },
    planeColor         = { 0.20, 0.34, 0.48 },
    baseColor          = { 0.40, 0.55, 0.68 },
    minColor           = { 0.10, 0.18, 0.26 },
    absorb             = { 0.004, 0.004, 0.002 },
    ambientFactor      = 1.0,
    diffuseFactor      = 1.0,
    specularFactor     = 1.0,
    specularPower      = 20,
    fresnelMin         = 0.2,
    fresnelMax         = 0.8,
    fresnelPower       = 4.0,
    reflectionDistortion = 1.0,
    blurBase           = 2.0,
    blurExponent       = 1.5,
    perlinStartFreq    = 8.0,
    perlinLacunarity   = 3.0,
    perlinAmplitude    = 0.9,
    numTiles           = 1,
    shoreWaves         = true,
    forceRendering     = false,
  },

  terrainTypes = {
    [0] = { name = "Default", hardness = 1.0, receiveTracks = true,
            moveSpeeds = { tank = 1.0, kbot = 1.0, hover = 1.0, ship = 1.0 } },
    [1] = { name = "Rock",    hardness = 2.0, receiveTracks = false,
            moveSpeeds = { tank = 0.85, kbot = 1.0, hover = 1.0, ship = 0.0 } },
    [2] = { name = "Sand",    hardness = 0.4, receiveTracks = true,
            moveSpeeds = { tank = 0.7,  kbot = 0.9, hover = 1.2, ship = 0.0 } },
    [3] = { name = "Water",   hardness = 0.1, receiveTracks = false,
            moveSpeeds = { tank = 0.0,  kbot = 0.0, hover = 1.0, ship = 1.0 } },
  },

  teams = {
    [0]  = { startPos = { x =  640, z =  640 } },
    [1]  = { startPos = { x = 1920, z =  640 } },
    [2]  = { startPos = { x = 3200, z =  640 } },
    [3]  = { startPos = { x = 4480, z =  640 } },
    [4]  = { startPos = { x = 5760, z =  640 } },
    [5]  = { startPos = { x = 7040, z =  640 } },
    [6]  = { startPos = { x = 8320, z =  640 } },
    [7]  = { startPos = { x = 9600, z =  640 } },
    [8]  = { startPos = { x =  640, z = 9600 } },
    [9]  = { startPos = { x = 1920, z = 9600 } },
    [10] = { startPos = { x = 3200, z = 9600 } },
    [11] = { startPos = { x = 4480, z = 9600 } },
    [12] = { startPos = { x = 5760, z = 9600 } },
    [13] = { startPos = { x = 7040, z = 9600 } },
    [14] = { startPos = { x = 8320, z = 9600 } },
    [15] = { startPos = { x = 9600, z = 9600 } },
  },

  custom = {
    fog = { color = { 0.78, 0.82, 0.88 }, height = 80, density = 0.4 },
  },
}

return mapinfo
```

Sidecar files the emitter must also write:

```lua
-- mapconfig/map_metal_layout.lua
return {
  spots = {
    { x =  640, z = 1280, metal = 2.0 },
    { x = 1920, z = 1280, metal = 2.0 },
    -- ... mirrored 16 base mexes + 8 mid-line ...
    { x = 5120, z = 5120, metal = 4.0 },
  },
  geos = {
    { x = 3200, z = 5120 },
    { x = 7040, z = 5120 },
  },
}
```

```lua
-- mapconfig/map_startboxes.lua
return {
  startboxes = {
    [0] = { boxes = { { { 0.00, 0.00 }, { 1.00, 0.12 } } }, startpoints = {} },
    [1] = { boxes = { { { 0.00, 0.88 }, { 1.00, 1.00 } } }, startpoints = {} },
  },
}
```

```lua
-- mapconfig/featureplacer/features.lua
return {
  { name = "geovent",                  x = 3200, z = 5120, rot = "0" },
  { name = "geovent",                  x = 7040, z = 5120, rot = "0" },
  { name = "allpinesb_ad0_greena_m",   x = 2100, z = 2200, rot = "8192"  },
  { name = "allpinesb_ad0_greenb_l",   x = 2300, z = 2400, rot = "16384" },
  -- ... etc ...
}
```

This four-file emit hits every consumer the engine and BAR-mod gadgets check. The "featureless untextured map" symptom in the current emitter is caused by missing `resources.detailTex`/`specularTex`/`splatDistrTex`, missing `lighting` ambient/diffuse values, and missing `depend = "Spring Bitmaps"`; not by missing `smf` keys.

## Caveats

- Several BAR-side gadget paths in this report could not be verified at exact filename+line in current `beyond-all-reason/Beyond-All-Reason` master through public web tooling alone; in particular `unit_sunfacing.lua` does not appear to exist on master (the editor team's claim may refer to an older fork or a Zero-K gadget). Treat the §7 sunDir landmine as defensive: emit it anyway. Run `git ls-files | xargs grep -nF 'mapinfo.'` against a local clone to confirm exact line numbers before pinning the schema.
- `rts/Map/MapInfo.cpp` and `MapInfo.h` could not be fetched directly through this tool's URL constraints; the field tables above are reconciled from Spring RTS engine version 105.0 (which Recoil forked from, per the official RecoilEngine wiki) plus the Mapdev wiki and observed BAR map behavior. The field *names* are stable; any default-value drift in Recoil's recent commits should be re-verified once on a local checkout.
- The Beherith gist at `gist.github.com/Beherith/97cae4d300e675ca261e661fc58266d1` referenced in the source list could not be retrieved during this pass; spot-check the schema against it before locking the Rust model.