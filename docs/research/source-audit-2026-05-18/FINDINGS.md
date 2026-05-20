# Source Audit — Gaps in our Map-Creation Understanding

**Status:** Landed 2026-05-18 — verified against locally-cloned BAR sources.
**Source clones used (under `/home/teague/code/`):**
- `RecoilEngine` at HEAD — `rts/Map/MapInfo.{h,cpp}`, `rts/Map/SMF/{SMFFormat.h, SMFMapFile.cpp, SMFRenderState.cpp}`, `rts/System/FileSystem/ArchiveScanner.cpp`, `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`.
- `Beyond-All-Reason` at HEAD (shallow) — `luarules/gadgets/map_metal_spot_placer.lua`, `unit_sunfacing.lua`, `common/upgets/api_resource_spot_finder.lua`, `luarules/configs/ffa_startpoints/`.
- `BYAR-Chobby` at HEAD — `LuaMenu/widgets/gui_maplist_panel.lua`.
- `maps-metadata` at HEAD — `schemas/map_list.yaml`, `map_list.yaml`.
- `springrts_smf_compiler` (PyMapConv) at HEAD — `src/pymapconv.py`.

Read order intent: cross-reference every existing `claude-research-findings.md`
claim against the actual source. List every disagreement and every gap.

---

## TL;DR — what changes in the editor

1. **`mapinfo.lighting` MUST carry both `sundir` (lowercase) and `sunDir`
   (camelCase) keys.** The engine reads camelCase only (`MapInfo.cpp:207`);
   BAR's active `unit_sunfacing.lua` gadget reads lowercase only. Writing
   only one of the two silently breaks half the system. This is a new
   load-bearing pitfall.
2. **`atmosphere.skyDir` is deprecated** — engine logs an `L_DEPRECATED`
   warning and uses `atmosphere.skyAxisAngle` (float4 = axis xyz + radians)
   instead. Existing research lists `skyDir`. Emitter should write
   `skyAxisAngle`.
3. **The DNTS shader does NOT require `specularTex`** to enable splat
   normal blending in current Recoil. The historical "silent disable
   without specularTex" claim collapses for the engine path, but the
   visual result still looks bad without spec — keep the lint warning
   reworded.
4. **Geo spots are NOT a `geos = {…}` array in `map_metal_layout.lua`.**
   BAR's `api_resource_spot_finder` derives geo positions exclusively
   from features with `geoThermal = true`. The Zero-K convention does
   NOT carry over. Emitter must place a `geovent` feature per geo, NOT
   write a `geos` table.
5. **`mapinfo.gui.minimapRotation` is unused** by current Recoil
   (`MapInfo::ReadGui` only reads `autoShowMetal`). Drop the field from
   the schema or document it as legacy-only.
6. **Splat-rendering shader composite math** has five hard corrections
   (constant name typo, tangent basis derivation, base-normal decoding,
   specular exponent formula, alpha-sign interpretation). Existing
   research's WGSL formula will render visibly wrong if implemented
   verbatim. Section 7 has the corrected math.
7. **`maps-metadata` schema** (the F20 publish-to-BAR target) has a
   24-entry terrain enum and a 4-entry gameType enum that the F1
   wizard's biome picker should align to. Currently we ship four
   biome presets keyed to our own names.
8. **modtype = 3** is one of six enum values (0=hidden, 1=primary,
   2=unused, 3=map, 4=base, 5=menu) per `ArchiveScanner.cpp:83`.
   Research treats it as a sentinel.
9. **`map.gravity`** is supplied in elmos/sec² in `mapinfo.lua`; the
   engine stores it negated and divided by `GAME_SPEED²`. The 130 BAR
   convention is the **input** value, not the stored one.

These nine bullets are the highest-leverage corrections. The rest of
this document is the full reconciliation.

---

## 1. mapinfo.lua schema — engine reader

The canonical reader is `rts/Map/MapInfo.cpp` (Recoil HEAD,
2026-05-18). It populates `CMapInfo` (`rts/Map/MapInfo.h`).
Below is the table reconciled with field-by-field source citations.
Where the engine struct field name differs from the Lua key,
**Lua key** is what the emitter must write.

### 1.1 top-level (read by `ReadGlobal`)

| Lua key | Default | Source line | Notes |
|---|---|---|---|
| `name` | (constructor arg) | line 50 | Passed in by archive scanner, not from `mapinfo.lua`. |
| `description` | `""` (falls back to `name`) | line 94 | |
| `author` | `""` | line 95 | |
| `maphardness` | `100.0` | line 97 | Clamped non-zero; sign preserved. |
| `notDeformable` | `false` | line 98 | |
| `gravity` | `130.0` (elmos/sec²) | line 100-102 | **TRANSFORMED**: engine stores `-gravity/(GAME_SPEED²)`. Emitter writes the input. |
| `tidalStrength` | `0.0` | line 104 | |
| `maxMetal` | `0.02` | line 105 | |
| `extractorRadius` | `500.0` | line 106 | BAR convention: **80**. |
| `voidAlphaMin` | `0.9` | line 107 | **NOT in current research.** Threshold for voidGround discard. |
| `voidWater` | `false` | line 108 | |
| `voidGround` | `false` | line 109 | |

### 1.2 archive scanner (NOT MapInfo.cpp)

Read by `rts/System/FileSystem/ArchiveScanner.cpp` (line 74-87 lists
the known tags). These are top-level keys, but they're consumed
**before** `MapInfo.cpp` runs.

| Lua key | Required? | Notes |
|---|---|---|
| `name` | **yes** | Display name; archive index uses it. |
| `shortname` | no | |
| `version` | no | |
| `mutator` | no | |
| `game` | no | |
| `shortgame` | no | |
| `description` | no | |
| `mapfile` | no | Engine source FIXME comment questions whether it's even used. |
| `modtype` | **yes** | Enum: `0`=hidden, `1`=primary, `2`=unused, `3`=**map**, `4`=base, `5`=menu. |
| `depend` | no (table) | BAR convention: `{"Map Helper v1"}`. Add `"Spring Bitmaps"` for shared textures. |
| `replace` | no (table) | |
| `onlyLocal` | no (bool) | If true, no incoming connections accepted. |

**Important divergence from current research:** research lists
`version` as "yes (Chobby)". The scanner declares it as `false` —
not required by the engine. Chobby may treat it as required for
display; needs a separate verification pass.

### 1.3 `atmosphere` (read by `ReadAtmosphere`, line 127-172)

| Lua key | Default | Source line | Notes |
|---|---|---|---|
| `minWind` | `5.0` | line 134 | Clamped non-negative. |
| `maxWind` | `25.0` | line 135 | `minWind` is clamped ≤ `maxWind`. |
| `fogStart` | `0.1` | line 137 | |
| `fogEnd` | `1.0` | line 138 | |
| `fogColor` | `(0.7, 0.7, 0.8)` | line 139 | |
| `skyBox` | `""` | line 141 | |
| `skyColor` | `(0.1, 0.15, 0.7)` | line 142 | |
| `skyDir` | **DEPRECATED** | line 144-146 | Engine logs `L_DEPRECATED` warning. Use `skyAxisAngle`. |
| `skyAxisAngle` | `{0, 0, 1, 0}` | line 149 | float4: xyz = rotation axis (normalized), w = angle in radians (clamped). |
| `sunColor` | `(1, 1, 1)` | line 162 | |
| `cloudColor` | `(1, 1, 1)` | line 163 | |
| `fluidDensity` | `0.3` (=1.2 × 0.25) | line 164 | **NOT in current research.** Atmosphere "air density" in kg/m³. |
| `cloudDensity` | `0.5` | line 165 | Clamped non-negative. |

### 1.4 `lighting` (read by `ReadLight`, line 201-225)

**WATCH** — Lua keys differ from the C++ struct field names:

| Lua key | C++ struct field | Default | Notes |
|---|---|---|---|
| `sunDir` | `light.sunDir` (float4) | `(0,1,2,1.0)` | **NOT** `(0,1,2,1e9)` as research claims. |
| `groundAmbientColor` | `groundAmbientColor` | `(0.5,0.5,0.5)` | |
| `groundDiffuseColor` | `groundDiffuseColor` | `(0.5,0.5,0.5)` | |
| `groundSpecularColor` | `groundSpecularColor` | `(0.1,0.1,0.1)` | |
| `groundShadowDensity` | `groundShadowDensity` | `0.8` | clamped [0,1]. |
| `unitAmbientColor` | `modelAmbientColor` | `(0.4,0.4,0.4)` | Lua key NOT `modelAmbientColor`. |
| `unitDiffuseColor` | `modelDiffuseColor` | `(0.7,0.7,0.7)` | |
| `unitSpecularColor` | `modelSpecularColor` | `= modelDiffuseColor` | |
| `unitShadowDensity` | `modelShadowDensity` | `0.8` | clamped [0,1]. |
| `specularExponent` | `specularExponent` | `100.0` | |

**Critical landmine — `sundir` vs `sunDir`:**
- Engine reads ONLY `sunDir` (camelCase) — `MapInfo.cpp:207`.
- BAR's active `unit_sunfacing.lua` gadget (March 2024) reads ONLY
  `sundir` (lowercase) — line 43.
- Lua tables are case-sensitive: these are TWO DISTINCT keys.
- **The emitter MUST write both keys** with the same value, or the
  gadget will silently dereference nil at unit construction time.

### 1.5 `water` (read by `ReadWater`, line 228-336)

The engine reads **far more** water keys than research lists. Full set:

| Lua key | Default | Notes |
|---|---|---|
| `fluidDensity` | `240.0` (= 960 × 0.25) | kg/m³. NOT in research. |
| `repeatX`, `repeatY` | `0.0`, `0.0` | Texture tile UVs. |
| `damage` | `0.0` | DPS, scaled by `(UNIT_SLOWUPDATE_RATE × INV_GAME_SPEED)`. |
| `absorb` | `(0,0,0)` | float3. |
| `baseColor` | `(0,0,0)` | float3. *Research says `(0.4,0.7,0.8)` — wrong.* |
| `minColor` | `(0,0,0)` | float3. *Research says `(0.1,0.2,0.3)` — wrong.* |
| `ambientFactor` | `1.0` | |
| `diffuseFactor` | `1.0` | |
| `specularFactor` | `1.0` | |
| `specularPower` | `20.0` | |
| `planeColor` | `(0.0, 0.4, 0.0)` float3 | Default is an actual color, NOT "unset". |
| `surfaceColor` | `(0.75, 0.8, 0.85)` | |
| `surfaceAlpha` | `0.55` | NOT in research. |
| `diffuseColor` | `(1,1,1)` | NOT in research. |
| `specularColor` | `= light.groundDiffuseColor` | |
| `fresnelMin` / `fresnelMax` / `fresnelPower` | `0.2` / `0.8` / `4.0` | |
| `reflectionDistortion` | `1.0` | Lua key is `reflectionDistortion`, struct is `reflDistortion`. |
| `blurBase` / `blurExponent` | `2.0` / `1.5` | |
| `perlinStartFreq` / `perlinLacunarity` / `perlinAmplitude` | `8.0` / `3.0` / `0.9` | |
| `windSpeed` | `1.0` | NOT in research. |
| `waveOffsetFactor` | `0.0` | NOT in research. |
| `waveLength` | `0.15` | NOT in research. |
| `waveFoamDistortion` | `0.05` | NOT in research. |
| `waveFoamIntensity` | `0.5` | NOT in research. |
| `causticsResolution` | `75.0` | NOT in research. |
| `causticsStrength` | `0.08` | NOT in research. |
| `texture` / `foamTexture` / `normalTexture` | `""` | Fall back to `resources.lua` `graphics.maps.watertex`/`waterfoamtex`/`waternormaltex`. |
| `numTiles` | `4` (when custom `normalTexture` absent), else `1` from `resources.lua` | clamped [1, 16]. Research says default 1 — wrong. |
| `shoreWaves` | `true` | |
| `forceRendering` | `false` | |
| `caustics` | (subtable) | If absent, engine loads 32 default caustic textures from `bitmaps/caustics/`. |

**`water.hasWaterPlane`** (struct flag, NOT a Lua key) is set if the
`planeColor` Lua key exists (presence, not value). This is the
mechanism behind the "voidWater requires omitting planeColor"
pitfall — confirmed.

### 1.6 `splats` (read by `ReadSplats`, line 175-182)

| Lua key | Default | Notes |
|---|---|---|
| `texScales` | `(0.02, 0.02, 0.02, 0.02)` | float4. |
| `texMults` | `(1, 1, 1, 1)` | float4. |

Matches research.

### 1.7 `grass` (read by `ReadGrass`, line 184-199)

| Lua key | Default | Notes |
|---|---|---|
| `bladeWaveScale` | `1.0` | If 0, grass doesn't sway. |
| `bladeWidth` | `0.7` | |
| `bladeHeight` | `4.5` | Actual = bladeHeight + rand(0, bladeHeight). |
| `bladeAngle` | `1.0` | |
| `maxStrawsPerTurf` | `150` | |
| `bladeColor` | `(0.1, 0.4, 0.1)` | **Lua key is `bladeColor`**, not `grass.color`. Research-incomplete. |

`resources.grassBladeTex` provides the bitmap (in `resources` subtable,
not `grass`).

### 1.8 `resources` + `smf` (read by `ReadSMF`, line 353-428)

`resources` keys (read from `mapinfo.resources` subtable):

| Lua key | Struct field | Notes |
|---|---|---|
| `detailTex` | `smf.detailTexName` | Falls back to `resources.lua` `graphics.maps.detailtex` → `detailtex2.bmp`. |
| `specularTex` | `specularTexName` | |
| `splatDetailTex` | `splatDetailTexName` | |
| `splatDistrTex` | `splatDistrTexName` | |
| `grassShadingTex` | `grassShadingTexName` | Defaults to minimap if empty. |
| `skyReflectModTex` | `skyReflectModTexName` | |
| `detailNormalTex` | `blendNormalsTexName` | **Lua key ≠ struct.** |
| `lightEmissionTex` | `lightEmissionTexName` | |
| `parallaxHeightTex` | `parallaxHeightTexName` | |
| `splatDetailNormalTex` (subtable) | `splatDetailNormalTexNames[]` | See below. |
| `splatDetailNormalTex1..4` (keyed) | same | Legacy form. |
| `splatDetailNormalDiffuseAlpha` | `smf.splatDetailNormalDiffuseAlpha` | bool. |

**`splatDetailNormalTex` has TWO accepted forms** (line 383-399):
```lua
-- Modern (preferred):
resources.splatDetailNormalTex = {
  "tex1.dds", "tex2.dds", "tex3.dds", "tex4.dds",
  alpha = true,
}
-- Legacy (numbered keys):
resources.splatDetailNormalTex1 = "tex1.dds"
resources.splatDetailNormalTex2 = "tex2.dds"
-- ...
resources.splatDetailNormalDiffuseAlpha = 1
```
The engine prefers the subtable form when present.

`smf` keys (read from `mapinfo.smf` subtable):

| Lua key | Notes |
|---|---|
| `minHeight` / `maxHeight` | Overrides SMF-baked min/max if key exists. |
| `minimapTex` | Override SMF-baked minimap. |
| `metalmapTex` | **Override SMF-baked metalmap.** NOT in research. |
| `typemapTex` | Override SMF-baked typemap. |
| `grassmapTex` | Override SMF-baked grassmap. |
| `smtFileName0`, `smtFileName1`, ... | Iterated by `IntToString(i, "smtFileName%i")` — line 421. |

### 1.9 `terrainTypes` (read by `ReadTerrainTypes`, line 431-457)

Loop over `i = 0..255`:

| Sub-key | Default | Notes |
|---|---|---|
| `name` | `"Default"` | |
| `hardness` | `1.0` | clamped > 0. |
| `receiveTracks` | `true` | |
| `moveSpeeds.tank` | `1.0` | |
| `moveSpeeds.kbot` | `1.0` | |
| `moveSpeeds.hover` | `1.0` | |
| `moveSpeeds.ship` | `1.0` | |

Matches research.

### 1.10 `pfs` (read by `ReadPFSConstants`, line 459-479)

**NOT in current research.** BAR's pathfinder is QTPFS; the engine
reads its tuning constants here.

| Lua key | Default | Notes |
|---|---|---|
| `pfs.qtpfsConstants.layersPerUpdate` | `5` | |
| `pfs.qtpfsConstants.maxTeamSearches` | `25` | |
| `pfs.qtpfsConstants.minNodeSizeX` / `minNodeSizeZ` | `8` / `8` | |
| `pfs.qtpfsConstants.maxNodeDepth` | `16` | |
| `pfs.qtpfsConstants.numSpeedModBins` | `10` | |
| `pfs.qtpfsConstants.minSpeedModVal` / `maxSpeedModVal` | `0.0` / `2.0` | |
| `pfs.qtpfsConstants.maxNodesSearched` | `0` | |
| `pfs.qtpfsConstants.maxRelativeNodesSearched` | `0.0` | |

Most BAR maps omit this entirely. Stage 1+ MVP can ignore.

### 1.11 `gui` (read by `ReadGui`, line 119-124)

| Lua key | Default | Notes |
|---|---|---|
| `autoShowMetal` | `true` | |

**Only one key.** Research's `gui.minimapRotation` is NOT read.
Drop from emitter schema.

### 1.12 `sound` (read by `ReadSound`, line 481-542)

| Lua key | Notes |
|---|---|
| `sound.preset` | OpenAL EFX preset name (e.g. `"default"`). |
| `sound.passfilter.<param>` | Per-parameter overrides (FLOAT). |
| `sound.reverb.<param>` | Per-parameter overrides (FLOAT / BOOL / float3 VECTOR). |

Rarely used in BAR. Out-of-scope for editor MVP.

### 1.13 `teams` (read by `MapParser`)

This is **NOT in `MapInfo.cpp`** — the `teams[i].startPos` table is
read by `MapParser` for the game-setup pass. Research correctly
identifies it. `allyTeam` is a `script.txt` field, not a `mapinfo.lua`
field — research correctly notes this.

### 1.14 `custom`

Free-form `mapinfo.custom` table is exposed to gadgets via
`Spring.GetMapOptions()` and direct `mapinfo.custom` reads. Engine
itself doesn't parse it.

---

## 2. SMF binary format constants

From `rts/Map/SMF/SMFFormat.h` (verified at HEAD):

| Constant | Value | Source |
|---|---|---|
| Tile pixel size | 32 | `SMFHeader.tilesize`, validated by `CheckHeader`. |
| Square size (texel-per-vertex) | 8 | `SMFHeader.squareSize`, validated. |
| `texelPerSquare` | 8 | validated. |
| Minimap size on disk | exactly 699,048 bytes | `MINIMAP_SIZE` macro. |
| Minimap mip levels | 9 | `MINIMAP_NUM_MIPMAP`. |
| Single tile DXT1 size | 680 bytes (512 + 128 + 32 + 8) | `SMALL_TILE_SIZE`. |
| `mapx`, `mapy` divisibility | "Must be divisible by 128" (header comment) | NOT enforced by `CheckHeader`. |
| `magic` string | `"spring map file"` | validated. |
| `version` | `1` | validated. |

**Implication for the editor:**
- Heightmap is `(mapy+1) × (mapx+1)` `uint16` shorts — read by `ReadHeightmap`.
- Metalmap and typemap are `(mapx/2) × (mapy/2)` `uint8`.
- Grass extra-header (`MEH_Vegetation`) is `(mapx/4) × (mapy/4)` `uint8`.

A 16×16-SMU map: mapx=mapy=1024 (`64*16`), heightmap 1025×1025, metal/type 512×512, grass 256×256.

**The "must be divisible by 128" constraint in the SRS is correct
guidance**, but the engine doesn't reject smaller maps that
happen to obey other constraints. This is upstream guidance, not
a runtime check. Validation lives in PyMapConv / mapconv variants.

---

## 3. modtype enum (archive scanner)

From `rts/System/FileSystem/ArchiveScanner.cpp:83`:

```
modtype: 0=hidden, 1=primary, (2=unused), 3=map, 4=base, 5=menu
```

The editor MUST emit `modtype = 3`. Other values:
- `1` = mod / game (BAR itself is modtype 1).
- `4` = base content (`maphelper.sdz`, `bitmaps.sdz`).
- `5` = menu (Chobby is modtype 5).

Current research treats modtype as a binary "map = 3 or invisible";
the full enum lets us emit better error messages
(*"modtype 4 = base content, this is reserved for engine archives"*).

---

## 4. Chobby map filter

From `BYAR-Chobby/LuaMenu/widgets/gui_maplist_panel.lua:1676` and `:1683`:

```lua
if info and info.modtype == 3 and not mapFuncs[info.name] then
    -- accepted
end
-- ...
if lobby.name == "singleplayer" or certification ~= "Unofficial" then
    -- show in multiplayer map list
end
```

**Confirmed:**
1. Chobby requires `modtype == 3` (filtered at archive level).
2. Maps with `certification == "Unofficial"` only appear in **singleplayer / Skirmish**.
3. The `certification` field comes from the `mapDetails` table loaded
   from `maps-metadata`, NOT from the `.sd7` itself — so the user's
   newly-built map is "Unofficial" until it lands in maps-metadata.

This matches research. No correction needed.

---

## 5. Metal spots — BAR vs research

From `Beyond-All-Reason/luarules/gadgets/map_metal_spot_placer.lua`
(by raaar, 2017):

```lua
local MAPSIDE_METALMAP = "mapconfig/map_metal_layout.lua"
local mapConfig = VFS.FileExists(MAPSIDE_METALMAP) and VFS.Include(MAPSIDE_METALMAP) or false

function gadget:Initialize()
    -- bail if SMF metalmap already has metal:
    if not hasMetalmap and mapConfig and Spring.GetGameFrame() == 0 then
        local spots = mapConfig.spots
        local metalFactor = 0.43 * 9 / 21
        for i = 1, #spots do
            local spot = spots[i]
            -- Place metal in a 5x5 cross pattern (corners excluded)
            -- centered at (spot.x / 16, spot.z / 16) metal-map cells.
            Spring.SetMetalAmount(xi, zi, spot.metal * metalFactor * 255)
        end
    end
end
```

**Conclusions:**

1. **Schema confirmed:** `mapconfig/map_metal_layout.lua` returning
   `{ spots = [{x, z, metal}, ...] }`. `x` and `z` in elmos; `metal`
   is the BAR-convention multiplier (e.g. `2.0` for a normal mex, `4.0`
   for a strong central mex).
2. **The gadget runs only at game frame 0** and bails if the SMF
   metalmap already has any non-zero pixel. So the editor's SMF
   metalmap should be **all-zero** (let PyMapConv bake a black
   metalmap PNG) when emitting Lua spots.
3. **Metal amount math:** `Spring.SetMetalAmount(xi, zi, spot.metal × 0.43 × 9/21 × 255)`.
   - For `spot.metal = 2.0`, peak cell value ≈ `100.5`.
   - The 5×5 cross pattern (21 cells) places identical density at every cell.
4. **`geos` table is NOT read by BAR** in this gadget. **Geo spots
   come from feature placement** (features with `geoThermal = true`,
   typically the `geovent` feature). The Zero-K convention `geos = {...}` in
   `map_metal_layout.lua` does NOT apply to BAR.
5. **`metalValueOverride` is NOT read** by BAR. Zero-K-only.

**Editor implication:** the `Project.geo_vents` model should map ONLY
to feature placements (mapconfig/featureplacer/features.lua) with the
canonical `geovent` name. There's no second array to emit.

---

## 6. Resource spot finder (BAR API)

From `Beyond-All-Reason/common/upgets/api_resource_spot_finder.lua`
(by Niobium / Tarte, last updated April 2022, GPL-2.0+):

```lua
function upget:Initialize()
    metalSpots, isMetalMap = GetSpotsMetal()   -- scans engine metalmap
    geoSpots = GetSpotsGeo()                   -- scans for geoThermal features
    globalScope["resource_spot_finder"] = {
        metalSpotsList = metalSpots,
        geoSpotsList = geoSpots,
        ...
    }
end
```

**Pipeline confirmed end-to-end:**

```
mapconfig/map_metal_layout.lua  (mapper-authored)
        │
        ▼
map_metal_spot_placer.lua  (BAR gadget, paints engine metalmap)
        │
        ▼
api_resource_spot_finder.lua  (BAR upget, re-derives spots from engine metalmap)
        │
        ▼
GG['resource_spot_finder'].metalSpotsList  (consumed by widgets)
```

For geo:

```
mapconfig/featureplacer/features.lua  (mapper-authored, places geovent features)
        │  (PyMapConv compiles into SMF feature placements)
        ▼
api_resource_spot_finder.GetSpotsGeo()  (scans Spring.GetAllFeatures() for FeatureDef.geoThermal)
        │
        ▼
GG['resource_spot_finder'].geoSpotsList
```

The editor's correct emitter behavior:
- **Metal:** `Project.metal_spots` → `mapconfig/map_metal_layout.lua` `spots = [{x, z, metal}]`. No engine metalmap.
- **Geo:** `Project.geo_vents` → `mapconfig/featureplacer/features.lua` with `name = "geovent"`. No `geos` table.

---

## 7. SMF fragment shader — composite math (corrections)

Source: `cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl` and
`rts/Map/SMF/SMFRenderState.cpp` at HEAD. Five corrections to the
splat-rendering research:

### 7.1 Constant name typo

```glsl
#define SMF_INTENSITY_MULT (210.0 / 255.0)   // shader line 4
```

The macro is `SMF_INTENSITY_MULT` (with `T`). Research and proposed
ADR-035 spell it `SMF_INTENSITY_MUL`. Fix the WGSL constant name and
the lint rule.

### 7.2 DNTS gating

```cpp
// SMFRenderState.cpp:114
glslShaders[n]->SetFlag("SMF_DETAIL_NORMAL_TEXTURE_SPLATTING",
    (smfMap->GetSplatDistrTexture() != 0 && smfMap->HaveSplatNormalTexture()));
```

DNTS is gated on `splatDistrTex != 0 && splatDetailNormalTex[].size() > 0`.
**`specularTex` is NOT in the gate condition**. The 2010-era forum
claim ("DNTS silently disables without specularTex") is no longer
true at the C++ render-state level.

However, the lint warning still belongs in the editor — without a
specular texture, the shader's specular branch falls back to a
constant `groundSpecularColor` and the visual result is muddier than
maps with spec. Reword the warning:

> *"No specular texture set. DNTS still renders, but the result looks
> noticeably flatter than published BAR maps; ship a 1024×1024 BC1
> specular even if it's mostly grey."*

### 7.3 DNTS composite math

```glsl
// SMFFragProg.glsl:174-198
vec4 GetSplatDetailTextureNormal(vec2 uv, out vec2 splatDetailStrength) {
    vec4 splatTexCoord0 = vertexWorldPos.xzxz * splatTexScales.rrgg;
    vec4 splatTexCoord1 = vertexWorldPos.xzxz * splatTexScales.bbaa;
    vec4 splatCofac = texture2D(splatDistrTex, uv) * splatTexMults;

    splatDetailStrength.x = min(1.0, dot(splatCofac, vec4(1.0)));

    vec4 splatDetailNormal;
    splatDetailNormal  = ((texture2D(splatDetailNormalTex1, splatTexCoord0.st) * 2.0 - 1.0) * splatCofac.r);
    splatDetailNormal += ((texture2D(splatDetailNormalTex2, splatTexCoord0.pq) * 2.0 - 1.0) * splatCofac.g);
    splatDetailNormal += ((texture2D(splatDetailNormalTex3, splatTexCoord1.st) * 2.0 - 1.0) * splatCofac.b);
    splatDetailNormal += ((texture2D(splatDetailNormalTex4, splatTexCoord1.pq) * 2.0 - 1.0) * splatCofac.a);

    splatDetailNormal.y = max(splatDetailNormal.y, 0.01);

    #ifdef SMF_DETAIL_NORMAL_DIFFUSE_ALPHA
        splatDetailStrength.y = clamp(splatDetailNormal.a, -1.0, 1.0);
    #endif
    return splatDetailNormal;
}
```

**Key corrections:**
- Each DNTS layer uses its own UV stream:
  - Layer 0: `worldPos.xz * splatTexScales.r` (or via `.st`)
  - Layer 1: `worldPos.xz * splatTexScales.g`
  - Layer 2: `worldPos.xz * splatTexScales.b`
  - Layer 3: `worldPos.xz * splatTexScales.a`
- `splatCofac = texture2D(splatDistrTex, uv) * splatTexMults` —
  the per-pixel splat distribution × the per-channel intensity, applied
  ONCE. NOT applied separately to normal vs diffuse.
- **The entire RGBA sample is decoded as signed** (`* 2.0 - 1.0`),
  including alpha. Then `* splatCofac[i]` is applied to the whole
  vec4, NOT just the rgb.
- The normal-blend strength (used as the `mix()` factor in the main
  shader) is `min(1.0, dot(splatCofac, vec4(1.0)))` — the **saturating
  sum of all 4 weights**.
- The diffuse offset is `clamp(splatDetailNormal.a, -1.0, 1.0)` —
  whatever the sum of `(α_i*2-1) * splatCofac[i]` came out to,
  clamped.
- `splatDetailNormal.y = max(splatDetailNormal.y, 0.01)` — clamps
  the Y component to avoid zero normal when all weights are zero.

### 7.4 Tangent space

```glsl
// SMFFragProg.glsl:276-278
vec3 tTangent = normalize(cross(normal, vec3(-1.0, 0.0, 0.0)));
vec3 sTangent = cross(normal, tTangent);
mat3 stnMatrix = mat3(sTangent, tTangent, normal);
```

The TBN basis is built from the **per-vertex/per-fragment `normal`
sampled from `normalsTex`**, NOT a static `T=+X, B=+Z, N=+Y` as the
research's WGSL uses. The DNTS detail normal is then `mix(normal,
normalize(stnMatrix * splatDetailNormal.xyz), splatDetailStrength.x)`.

### 7.5 Base normal decoding

```glsl
// SMFFragProg.glsl:146-150
vec3 GetFragmentNormal(vec2 uv) {
    vec3 normal;
    normal.xz = texture2D(normalsTex, uv).ra;    // X=red, Z=alpha
    normal.y  = sqrt(1.0 - dot(normal.xz, normal.xz));
    return normal;
}
```

The base map normal is reconstructed from **only the R and A
channels** of `normalsTex`. The G and B channels are unused. Y is
derived. This is a Spring-specific encoding optimized for BC3 — the
editor's normal-generation pipeline must produce textures in this
format (R = X, A = Z), not generic RGB normals.

### 7.6 Specular exponent

```glsl
// SMFFragProg.glsl:412-413
#ifdef SMF_SPECULAR_LIGHTING
    float specularExp  = specularCol.a * 16.0;
#else
    float specularExp  = groundSpecularExponent;
#endif
```

When `specularTex` exists, the exponent is `α * 16.0` — flat
multiplication. Research's `mix(16.0, specularExponent, alpha)` is
wrong; the global `lighting.specularExponent` uniform is only used
when NO specular texture is bound.

### 7.7 Final color composition

```glsl
// SMFFragProg.glsl:381
fragColor.rgb = (diffuseCol.rgb + detailCol.rgb) * shadeInt.rgb;
// later:
fragColor.rgb += specularInt;
```

`detailCol` from DNTS is `vec4(splatDetailStrength.y)` — a single
greyscale channel. It is ADDED to `diffuseCol.rgb`, then the sum is
multiplied by `shadeInt.rgb` (ambient + Lambert × shadow). Specular
is added afterwards.

---

## 8. FFA start positions (BAR-specific)

From `Beyond-All-Reason/luarules/configs/ffa_startpoints/README.md`
and sample files (e.g. `Altair Crossing.lua`):

```lua
local startPoints = {
  [1] = { x = 455, z = 918, },   -- corner 1
  [2] = { x = 3629, z = 855, },  -- corner 2
  -- ...
}

local byAllyTeamCount = {
  [4] = { { 1, 2, 3, 4 } },      -- 4-way: corners
}

return {
  startPoints = startPoints,
  byAllyTeamCount = byAllyTeamCount,
}
```

**Two locations** the FFA gadget looks for these:
1. **BAR mod repo:** `luarules/configs/ffa_startpoints/<MapNameSubstring>.lua`
   (case-insensitive, space-insensitive filename match against map name).
2. **Map archive:** `luarules/configs/ffa_startpoints.lua` (single file
   at fixed path inside `.sd7`).

If both exist, BAR's bundled config wins. The map's
`mapinfo.teams[i].startPos` is the third fallback.

**Editor implication for F8 (start positions):** the editor's
`Project.ally_groups` data model can also emit this FFA format when
`gameType` includes `ffa`. Track in F8 v2 or F20 publish-to-BAR
workflow. Not blocking for MVP.

---

## 9. maps-metadata schema (the F20 target)

From `maps-metadata/schemas/map_list.yaml`:

Per-map yaml object fields:

| Field | Type | Required | Notes |
|---|---|---|---|
| `springName` | string | yes | The `info.name` after archive scan. |
| `displayName` | string | yes | Free-form human name. |
| `author` | string | yes | |
| `title` | string | no | |
| `description` | string | no | |
| `gameType` | string[] enum | yes | `ffa` / `1v1` / `team` / `pve`. |
| `terrain` | string[] enum | yes | See below. |
| `playerCount` | int | yes | Maximum supported. |
| `teamCount` | int | yes | Maximum supported. |
| `certified` | bool | yes (default false) | Curated by humans. |
| `inPool` | bool | yes (default false) | In the rotating map pool. |
| `special` | string | no | Special-mode tag. |
| `photo` | uploadedFile[] | yes | 1 photo max. |
| `backgroundImage` | uploadedFile[] | yes (default []) | 1 max. |
| `perspectiveShot` | uploadedFile[] | yes (default []) | 1 max. |
| `inGameShots` | uploadedFile[] | yes (default []) | Multiple. |
| `mapLists` | string[] | no (default []) | Membership in named lists. |
| `startboxesSet` | object | no | Keyed by player-count strings. |
| `startPos` | object | no | FFA-style position config. |
| `startPosActive` | bool | no (default false) | |
| `minPlayerCount` | int | no | |

**Terrain enum** (24 values, used for filtering in Chobby):
```
lava, ice, acidic, alien, asteroid, space,
desert, forests, grassy, tropical, swamp, jungle, wasteland,
metal, industrial, ruins,
sea, water, island, shallows,
chokepoints, asymmetrical, flat, hills
```

**gameType enum:** `ffa`, `1v1`, `team`, `pve`.

**Startbox polygon coordinates** are in `[0..200]` (NOT `[0..1]` as
the editor currently assumes) — `x` and `y` are integer in `0..200`
representing percent×2 of map width/height. This is the lobby's
canonical format.

**Implication for the editor:**
- F1 wizard's biome dropdown should map to the maps-metadata `terrain`
  enum (or a subset).
- F20 "Publish to BAR" generates a yaml row matching this schema.
- The startbox polygon scale conversion needs a sanity check —
  current `Project.ally_groups[*].box_polygon` (B6 / ADR-032) uses
  `0..1` fractions per the editor's own convention, but emission to
  `mapconfig/map_startboxes.lua` should produce the same `0..200`
  integer space the maps-metadata schema uses for consistency.

---

## 10. PyMapConv responsibilities (clarification)

From `springrts_smf_compiler/src/pymapconv.py`:

PyMapConv **does NOT touch `mapinfo.lua`**. It produces:
- `.smf` binary (header + heightmap + typemap + tile indices + minimap + metalmap)
- `.smt` tile file (DXT1 tiles)
- `_featureplacement.lua` (line 1278) — feature data baked into the
  SMF feature header

**Implication:** the editor's `mapinfo.lua` emitter, metal layout, and
startbox files are all entirely the editor's responsibility. PyMapConv
is purely the texture/heightmap compiler.

The `_featureplacement.lua` PyMapConv writes is a **separate path**
from `mapconfig/featureplacer/features.lua`. PyMapConv's file gets
baked into the SMF binary (via `MapFeatureStruct` records, see
`SMFFormat.h`); the `mapconfig/featureplacer/features.lua` path is
loaded at runtime by feature-placer-style gadgets. Both work; the
editor currently emits the Lua sidecar (per the existing
three-file convention).

---

## 11. Heightmap dimension constraints

From `rts/Map/SMF/SMFFormat.h:55-56`:

> `mapx`: Must be divisible by 128.
> `mapy`: Must be divisible by 128.

**With `squareSize = 8`:**
- `mapx` in heightmap squares = SMU count × 64
- For `mapx % 128 == 0`, SMU count must be even (2, 4, 6, ...).

`CheckHeader` (`SMFMapFile.cpp:16-28`) does NOT enforce this — only
magic / version / tilesize. So odd SMU counts technically work at the
binary level but were never tested by the upstream tools. **The
editor's wizard should still enforce even SMU counts** as a sanity
constraint.

---

## 12. Critical landmines (added to PITFALLS)

Synthesizing this audit's new findings into pitfalls that should
move into `docs/PITFALLS.md`:

### NEW-1 — `sundir` vs `sunDir` case mismatch
Engine reads `sunDir` (camelCase). BAR's `unit_sunfacing.lua` gadget
reads `sundir` (lowercase). Lua tables are case-sensitive. Emit
BOTH keys. (See §1.4.)

### NEW-2 — `atmosphere.skyDir` is deprecated
Use `atmosphere.skyAxisAngle` (float4: axis xyz + radians angle).
Engine logs `L_DEPRECATED` warning otherwise. (See §1.3.)

### NEW-3 — `geos = {...}` in `map_metal_layout.lua` is Zero-K, not BAR
BAR derives geo positions from features with `geoThermal = true`. Do
not write a `geos` array; emit `geovent` features instead. (See §5.)

### NEW-4 — SMF metalmap MUST be all-zero when using Lua spots
`map_metal_spot_placer.lua` bails if ANY metalmap pixel is non-zero.
PyMapConv must receive a black metalmap PNG. (See §5.)

### NEW-5 — `numTiles` defaults to 4, not 1, when water normal absent
A common BAR override is `numTiles = 1` for static water; the
default behaviour is 4 tiles assuming the standard 4x4 normal atlas.
(See §1.5.)

### NEW-6 — `light_t.sunDir.w = 1.0`, not `1e9`
Engine default is `(0, 1, 2, 1.0)`. Research's `1e9` is wrong; the
fourth component is just an intensity scalar, not a sunStartDistance.
(See §1.4.)

### NEW-7 — Specular exponent formula is `α × 16`, not a `mix`
With a specular texture present, the exponent is the alpha channel
times 16 — flat multiplication. `lighting.specularExponent` is only
consulted when NO specularTex is bound. (See §7.6.)

### NEW-8 — SMT base normal uses R+A channels only
The map normal texture's encoding is `R = nx`, `A = nz`, with Y
derived. Generic RGB normals will render incorrectly. (See §7.5.)

### NEW-9 — `gui.minimapRotation` is unused
Engine reads only `gui.autoShowMetal`. Existing research's mention
of `minimapRotation` is stale. (See §1.11.)

### NEW-10 — `voidAlphaMin` exists
`map.voidAlphaMin` (default `0.9`) controls voidGround's diffuse
alpha threshold. Not currently in any emitter. (See §1.1.)

---

## 13. What's STILL not audited

These warrant their own follow-up passes:

- **Decompile path** — research mentioned PyMapConv decompile;
  `fast_decompiler.py` exists in PyMapConv tree but wasn't read this
  pass. F13 (decompile / import existing `.sd7`) will need it.
- **Lobby parsing of `mapinfo.lua`** — there are downstream lobby
  consumers (TASClient, Chobby, SPADS autohost) that may have
  their own minimum-field expectations beyond what the engine reads.
  Not pursued.
- **`api_resource_spot_finder.GetSpotsMetal()`** — the algorithm that
  re-derives metal spot polygons from the engine metalmap pixels.
  Relevant if we ever want metal spots WITHOUT shipping the Lua
  sidecar.
- **`mapfeatures` repo** — `github.com/beyond-all-reason/mapfeatures`
  is the canonical source of stock feature names. Not cloned this
  pass; the editor's feature picker UI (Phase 4 / F7) will need its
  manifest.
- **`lib_startpoint_guesser.lua`** — `common/lib_startpoint_guesser.lua`
  in BAR appears to be related to auto-placing units at game start.
  Worth a read for F8 polish.

---

## Caveats

- All file lookups were performed against shallow / HEAD clones on
  2026-05-18. Active development on `beyond-all-reason/RecoilEngine`
  may shift defaults; re-verify before each major sprint.
- Several pitfalls in the existing research (e.g. fogStart==fogEnd
  breaks build ETA, splatDetailNormalTex requires specularTex)
  could not be re-tested in-engine and were taken on community
  authority. The audit confirms the engine-side gating where source
  was available; the community-anecdote ones remain unverified.
- `unit_sunfacing.lua` is the SECOND gadget in BAR known to read
  mapinfo subtables directly (the first is the dual-fog gadget per
  earlier research). There are likely more — `grep -rn '^[^-]*VFS\.Include.*mapinfo' luarules/`
  in BAR turns up only the sunfacing match against synced gadgets,
  but unsynced widgets weren't grepped this pass.
