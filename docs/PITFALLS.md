# Pitfalls

The ten silent failure modes from SRS §2.1, restated as engineering rules with
the test or invariant that catches each one.

## 1. Texture pipeline memory

A 16×16 map = 8192² diffuse (256 MB RGBA) + 8192² normal (256 MB) + 4096²
splat distribution (64 MB). Snapshot-undo of full images blows past 4 GB.

**Rule:** Edit buffers are tiled 256×256 chunks, copy-on-write, disk-backed
LRU. Undo deltas are *per dirty tile*, never full snapshots.

## 2. DXT1 is slow and lossy

Quality-tuned compression of a 16×16 takes 1–10 min. SMT mandates DXT1 — BC7
is not an option.

**Rule:** In-process BC1 (texpresso/bcdec/ISPC) for live preview. PyMapConv +
Compressonator for final-quality `.smt`. (Note: PyMapConv switched off
nvdxt.exe to Compressonator some time before May 2026 — no Wine needed on
Linux.) **CompressonatorCLI is invoked by name** (no path override in
upstream `src/pymapconv.py`); we vendor it under `tools/compressonator/`
and prepend that dir to `PATH` for the subprocess (ADR-014).

## 3. SMT tile dedup

Naïve SMT output is ~4× larger than tuned output. PyMapConv has the hash
deduplicator; if we ever fork, port verbatim.

**Rule:** Don't reimplement. If a fork is forced, copy the hash table
implementation byte-for-byte and reference the upstream SHA.

## 4. Heightmap edge constraint — `64·N + 1`

The #1 silent corruption. PyMapConv warns + resizes; user sees wrong terrain.

**Rule:** `MapSize::heightmap_dims()` is the only place dims are computed.
Any image import path rejects (with explicit error) — never silently crops or
pads. Unit test in `crates/barme-core/src/map_size.rs` pins the math.

## 5. Coordinate sign flips

Spring: Y-up, left-handed. Heightmap pixel `(x, y)` → world `(x·8, h, y·8)`.
Lua features use `{x, z, rot}` in elmos. The legacy `-i / --invert` flag
exists because of historical row-order confusion.

**Rule:** A single internal coordinate convention, documented in
`docs/ARCHITECTURE.md`. All converters live in one module. No ad-hoc flips.

## 6. `mapinfo.lua` silent dependencies

- `splatDetailNormalTex` without `specularTex` produces visibly flat
  output (FINDINGS §7.2 — earlier "silently disables" wording is wrong
  at the C++ render-state level; engine gates DNTS on
  `splatDistrTex && splatDetailNormalTex[].size() > 0`, not on spec.
  Spec absence still makes the map look noticeably worse than
  published BAR maps; the lint stays as a yellow warning, reworded.)
- `voidWater` requires unsetting `water.planeColor`
- Missing/renamed `smtFileName0` → the pink map
- `fogStart == fogEnd` breaks the ground-grid renderer

**Rule:** Linter pass before every save in `barme-mapinfo`. Each of these is
a named lint with a test fixture.

## 7. Pink-map trap on rename

Modern Recoil reads `mapinfo.smf.smtFileName0`. The SMT filename is no longer
hardcoded into the SMF, but if `mapinfo.lua` isn't rewritten on rename →
pink.

**Rule:** Rename is a single atomic operation that rewrites BOTH the SMT
filename and the matching `mapinfo.lua` entry.

## 8. DNTS + water + LOS animated-snow bug

`minHeight < 0` + DNTS + a Lua widget that touches LOS → TV-snow artifact
(Beherith, springrts forum t=35202).

**Rule:** Warn (don't block) when DNTS is enabled on a map with
`minHeight < 0`. Surface in the linter as a yellow warning.

## 9. `.sd7` solidity

7-Zip *solid* archives are silently rejected by SpringFiles indexing.

**Rule:** Packager invokes `7z` with `-ms=off`. Integration test opens the
output and asserts `IsSolid == false`.

## 10. PyMapConv license / redistribution

SRS flagged this as unresolved. **As of May 2026, PyMapConv ships with a
CC0-1.0 LICENSE** — redistribution is unrestricted. This pitfall is
historically interesting but no longer a blocker; we still verify the
LICENSE file is present in each vendored release.

**Rule:** The vendor script asserts a LICENSE file exists in the downloaded
PyMapConv archive and that its SPDX identifier is permissive.

---

## Bonus (not numbered but cited in SRS §2.1)

- **3D preview ≠ in-game.** Document the gap up front; do not pretend WYSIWYG.
- **Decompilation fidelity.** Round-trip loses diffuse precision (DXT1).
  Heightmap, metal, type, mapinfo are exact. Reuse PyMapConv's decompile path.
- **GPU brush latency.** Heightmap lives on the GPU as an R16 storage texture,
  edited by compute shaders. Read-back to CPU only on save.

## PyMapConv v0.6.3 Linux runtime quirks (found in Stage 0, ADR-014)

- **Always pass `-q 1` on Linux.** Default `numthreads=4` triggers an
  upstream read-back bug: tile compression writes flat into
  `temp/temp{i}.dds`, but the read-back loop checks `numthreads > 1`
  and tries `temp/thread{n}/temp{i}.dds` (the Windows multi-thread
  layout that Linux never creates). Crash:
  `FileNotFoundError: temp/thread0/temp0.dds`. Source: v0.6.3
  `src/pymapconv.py` lines 960–986.
  **Rule:** the driver passes `-q 1` unconditionally on Linux.

- **Trust artifact presence, not exit code.** PyMapConv exits with
  status 1 on Linux even after `All Done!` — the bundled Qt event loop
  closes "abnormally" when no display is held open. The contract is
  what's on disk (`.smf` + `.smt`).
  **Rule:** treat artifact-presence as success and log non-zero exit
  at `warn`. Only fail when artifacts are missing AND exit was
  non-zero.

## BAR Chobby + mod-gadget mapinfo expectations (found in Stage 0, goal #7)

The "engine-documented minimum" mapinfo is **not** the "real-world
minimum to play a BAR map." Three discrete gates a `.sd7` must clear,
each with different requirements:

### A. Engine scanner — extremely lax

`name`, `smf.smtFileName0`, and `teams[*].startPos` are the only
strictly required fields per the BAR map archive format reference (gist:
`burnhamrobertp/97cae4d300e675ca261e661fc58266d1`, "bare-minimum viable
map"). Everything else has engine defaults.

### B. Chobby map browser — filters on certification + modtype

`gui_maplist_panel.lua` in `beyond-all-reason/BYAR-Chobby` filters maps
by `info.modtype == 3` AND by a hardcoded "certified maps" list
shipped inside Chobby. Maps not in the list get
`certification = "Unofficial"`.

**Rule:** unofficial maps **only appear in Skirmish / singleplayer
lobbies**. Multiplayer lobbies hide them entirely. The
`[Chobby] Warning: GetMinimapImage not found for, <name>` warning is
benign — `api_map_handler.lua` auto-extracts the minimap from the SMF on
first scan; the warning fires once before extraction completes.

### C. BAR mod gadgets — fragile reads with no nil guards

BAR's mod-side Lua gadgets read mapinfo fields directly without
nil-checking the subtables. The first one we hit:
`luarules/gadgets/unit_sunfacing.lua` line 44:
```lua
sundir = mapinfo.lighting.sundir
```
With no `lighting` subtable in mapinfo, this throws
`attempt to index field 'lighting' (a nil value)` during the LuaRules
load phase — game appears to start but waiting-for-players hangs forever
because the synced state never completes.

**Rule:** the emitter must include the conventional subtables (at
minimum `lighting = { sundir = {…} }`, likely also `atmosphere`,
`water`, `terrainTypes`) even though the *engine* has defaults for
them. The list of subtables to include grows as we discover more
gadgets with this pattern — when a new crash surfaces, add the
required field with a sensible default and a regression test in
`barme-pipeline::mapinfo::tests`.

Reference: a complete in-the-wild example is
`scratch/bar-maps/extracted/titanduel/mapinfo.lua` (gitignored — copy
from `~/.local/state/Beyond All Reason/maps/titanduel_v3.sd7` to
inspect).

## Source-audit additions (2026-05-18)

Cross-referenced against locally-cloned `RecoilEngine`,
`Beyond-All-Reason`, `BYAR-Chobby`, `maps-metadata`, and
`springrts_smf_compiler` repos. Full write-up at
`docs/research/source-audit-2026-05-18/FINDINGS.md`. Ten new rules,
ordered by blast radius:

### 11. `sundir` vs `sunDir` case mismatch

- Engine (`rts/Map/MapInfo.cpp:207`) reads ONLY camelCase
  `lighting.sunDir`.
- BAR's active `luarules/gadgets/unit_sunfacing.lua` (March 2024,
  line 43) reads ONLY lowercase `lighting.sundir`.
- Lua tables are case-sensitive — these are two distinct keys.

**Rule:** the emitter writes BOTH keys with the same value into the
`lighting` subtable. Unit test in `barme-pipeline::mapinfo::tests`
asserts both keys appear in the rendered output.

### 12. `atmosphere.skyDir` is deprecated

Engine reader at `MapInfo.cpp:144-146` logs `L_DEPRECATED` if `skyDir`
is set; the field has been replaced by `skyAxisAngle` (float4: xyz
axis + radians angle).

**Rule:** emit `atmosphere.skyAxisAngle = {0, 0, 1, 0}` by default.
Never emit `atmosphere.skyDir`. Lint warning if a user-edited
override sets `skyDir`.

### 13. SMF metalmap must be all-zero when emitting Lua metal spots

`map_metal_spot_placer.lua` (BAR gadget) iterates the engine metalmap
at startup; if ANY pixel is non-zero, the gadget bails and the
Lua-defined spots are ignored.

**Rule:** the build pipeline ships a 1×1 black metalmap PNG to
PyMapConv whenever `Project.metal_spots` is non-empty. Integration
test: after build, load the `.sd7`'s SMF, read the metalmap region,
assert all bytes are zero.

### 14. Geo vents are NOT a `geos = {…}` array

The "second array in map_metal_layout.lua" pattern is Zero-K
convention; BAR's `api_resource_spot_finder.GetSpotsGeo()` scans the
map for engine features with `FeatureDef.geoThermal = true` (typically
the stock `geovent` feature).

**Rule:** `Project.geo_vents` emits ONLY into the Springboard
featureplacer trio (see §21) with `name = "geovent"`. Never write a
`geos` array in `map_metal_layout.lua` — that file holds `spots`
only.

> **2026-05-19 correction:** the original wording of this pitfall
> said "emits into `mapconfig/featureplacer/features.lua`". That
> path is also wrong — see §21 for the actual Springboard
> convention BAR maps use.

### 15. `splatDetailNormalTex` prefers the subtable form

The engine reader (`MapInfo.cpp:383-399`) checks for the subtable
form first and only falls back to the legacy numbered keys if
absent. Mixing both will result in the subtable form winning.

**Rule:** emit the subtable form only:
```lua
resources.splatDetailNormalTex = {
  "tex1.dds", "tex2.dds", "tex3.dds", "tex4.dds",
  alpha = (true|false),  -- == splatDetailNormalDiffuseAlpha
}
```

### 16. SMT base normal encoding (R + A channels only)

Map normal textures consumed by `SMFFragProg.glsl::GetFragmentNormal`
encode `nx` in R and `nz` in A. Y is reconstructed at sample time.
Generic RGB normal maps will render incorrectly.

**Rule:** the normal-bake pipeline (D-stream) targets BC3 / DXT5 with
R = nx, A = nz, G/B unused. Bench test: render a known sloped surface
and assert sun-incidence matches Recoil's reference within ε.

### 17. Specular exponent is `α × 16`, not a `mix`

With a specular texture bound, the fragment shader computes
`specularExp = specularCol.a * 16.0` (flat multiplication). The
`lighting.specularExponent` Lua field is only consulted when NO
specular texture is loaded.

**Rule:** the editor's specular-texture baker treats the alpha
channel as `desiredExponent / 16.0`, clamped to [0, 1]. Document in
the F4 splat / specular emission pipeline.

### 18. `lighting.sunDir.w = 1.0` default (NOT `1e9`)

Engine default is `float4(0.0f, 1.0f, 2.0f, 1.0f)` — the W
component is `1.0`, an intensity scalar. Earlier research's `1e9`
default is a sunStartDistance leakage from a different code path
and would over-saturate sunlight on map load if emitted.

**Rule:** `MapInfo::bar_default()`'s `lighting.sun_dir` uses
`[0.5, 0.7, 0.5, 1.0]` (or any normalized direction + W=1.0).
Unit test pins W to exactly 1.0.

### 19. `gui.minimapRotation` is unused

Engine reader at `MapInfo.cpp:119-124` (ReadGui) reads only
`autoShowMetal`. The `minimapRotation` field appears in older
wiki docs but is not consumed by the current Recoil renderer.

**Rule:** the C3 emitter omits `gui.minimapRotation` from its
defaults. The F9 form editor may surface a "show in raw Lua only"
toggle for legacy compatibility.

### 20. `map.voidAlphaMin` exists (default 0.9)

Top-level `voidAlphaMin` controls the alpha threshold below which
voidGround discards fragments (`MapInfo.cpp:107`). Not currently in
the schema. When users enable `voidGround`, they may want to tune
this.

**Rule:** add `voidAlphaMin: f32` (default 0.9) to the typed schema
in `barme-core::mapinfo_schema::MapBlock`. F9 surfaces it only when
`voidGround = true`.

### 21. `modtype` is a six-value enum

Values: 0=hidden, 1=primary, 2=unused, 3=map, 4=base, 5=menu.
Editor emits 3 unconditionally; the linter can reject user attempts
to set anything else.

**Rule:** `MapInfo::bar_default().modtype` is a typed `Modtype::Map`
enum, not a free `i32`. Serializes to `3`.

## D2 DNTS bake additions (2026-05-18, ADR-026)

### 22. JPG normal maps silently destroy X/Y vectors

JPEG 4:2:0 chroma subsampling drops half of the chroma data — including
the X (R) / Y (G) channels of a tangent-space normal map. The
ambientCG `*_NormalGL.zip` packs ship JPEG-encoded normals, so
"download whatever, the bake will figure it out" is silent-failure.

**Rule:** the texture-pack fetch script (`scripts/fetch-textures.sh`)
extracts `_1K-PNG.zip` only; the D2 bake (`bake_dnts`) probes
`normal.png` first, errors with `DntsBakeError::NormalNotPng` if only
`normal.jpg` is present. Per-slot dirname check enforced at
registry-scan time (D3 layer / Sprint 9). Unit test
`barme_pipeline::dnts::tests::rejects_jpg_normal` pins the guard.

### 23. Y-flip silent-failure on DNTS normals

The DNTS shader decodes per-fragment normals as `* 2 - 1` and builds
the TBN with +Y up (`SMFFragProg.glsl:276-278` per FINDINGS §7.4).
Sources authored under DirectX convention (Substance, Quixel,
Beherith's `*_flipped.dds`) need the green channel inverted before
shipping or every DNTS layer's lighting is upside-down on slopes —
visually subtle but reproducible.

**Rule:** `BakeOptions::yflip_normal` toggles the flip. Default OFF
because the ADR-025 starter pack ships ambientCG `*_NormalGL.png`
(already OpenGL). F23 (user-import, Phase 6) surfaces a per-import
override; convention is to ask once at import time rather than guess
from filename heuristics. Unit tests
`flip_green_inverts_and_preserves_other_channels` and
`flip_green_at_boundaries` pin the math; `passthrough_is_identity_
when_flag_off` pins the off-branch.

### 24. DNTS bake cache key MUST fold BakeOptions

Content-addressed caching is great until you forget to fold the
options into the cache key. Toggling `diffuse_in_alpha` or
`yflip_normal` on the same source bytes is a different DDS; missing
that in the key returns a stale cached entry that doesn't match
the active options.

**Rule:** `cache_key()` = `sha256(diffuse_bytes ‖ normal_bytes ‖
BakeOptions::to_cache_bytes())`. Each new `BakeOptions` field appends
to `to_cache_bytes` in a fixed position; the `cache_bytes_encode_
each_flag_in_a_distinct_position` test pins the encoding. Cache
files live at `tools/textures-cache/<sha>.dds` (gitignored).


### 21. Features need the Springboard featureplacer trio, not a bare Lua file

BAR has **no gadget** reading `mapconfig/featureplacer/features.lua`
(verified by direct grep across the full `Beyond-All-Reason` checkout
on 2026-05-19 — zero consumers in `luarules/`, `luaui/`, or `common/`).
Pre-2026-05-19 the editor emitted exactly that path; the file shipped
inside the `.sd7` and BAR silently ignored it. Geo vents authored in
the editor never spawned in-game.

Real BAR maps (`gecko_isle_remake_v1.2.1`, `jade_empress_1.3`,
`titanduel_v3`, …) ship the **Springboard featureplacer trio** —
the PD-licensed Gnome / Smoth gadget from August 2008, distributed
as map-bundled cargo because BAR doesn't ship it mod-side:

```
LuaGaia/Gadgets/FP_featureplacer.lua          ← gadget, map-bundled
mapconfig/featureplacer/config.lua            ← VFS.Include redirect
mapconfig/featureplacer/set.lua               ← the data
```

`config.lua` is a one-liner:

```lua
return VFS.Include("mapconfig/featureplacer/set.lua")
```

`set.lua` returns a single keyed table:

```lua
local setcfg = {
  unitlist = {},
  buildinglist = {},
  objectlist = {
    { name = "geovent", x = 4096, z = 4096, rot = 0 },
    { name = "agorm_talltree6", x = 224, z = 3616, rot = -23991 },
  },
}
return setcfg
```

Notes on the schema:

- **`rot` is an unquoted integer** in Spring heading units
  (`-32768..32767`). The gadget calls
  `Spring.CreateFeature(name, x, GroundHeight(x, z) + 5, z, rot)`
  which expects a number. PyMapConv's `-k` text-file format uses
  the quoted-string form (`rot = "0"`) but that's a separate
  codepath we don't use.
- **No `y` field.** The gadget samples
  `Spring.GetGroundHeight(x, z) + 5` at spawn so features ride
  the live terrain — sculpting the heightmap after authoring
  features does not detach them.
- **`unitlist` / `buildinglist`** are reserved for map-side
  pre-placed gaia units (campaign missions). Empty for normal
  competitive maps.

**Rule:** the build pipeline stages all three files. The gadget
itself is vendored at `crates/barme-pipeline/assets/FP_featureplacer.lua`
(PD license verified). `Project.geo_vents` populates `objectlist`
in `set.lua`; the future general-feature pipeline (C6 / Sprint 12)
appends to the same list.

### 22. `mapinfo.maxMetal` is a metal-yield scale, not a normalisation cap

The mapinfo field `maxMetal` is the m/s metal yield at full (`1.0`)
ground-metal saturation. The `gui_metalspots` widget computes
predicted F4 income as roughly `spot.worth * incomeMultiplier / 1000`
where `spot.worth` aggregates per-cell ground-metal × `maxMetal`
across the spot's cluster. Setting `maxMetal` too low scales every
spot's displayed value linearly down.

Real BAR maps cluster in `0.93..=4.11`:

```
jade_empress_1.3      maxMetal = 0.99
titanduel_v3          maxMetal = 1.26
supreme_isthmus_v2.1  maxMetal = 0.93
ravaged_remake_v1.2   maxMetal = 1.05
starwatcher_1.0       maxMetal = 4.11
```

The editor's pre-2026-05-19 default of `0.02` made a canonical
metal=2.0 spot display as `~0.1` m/s in F4 (50× too low).

**Rule:** `MapInfo::bar_default().max_metal = Some(1.0)`. The F9
form should expose this so map authors can match BAR's median
behaviour without per-project tuning. Linter (C8) warns if user
overrides drop below `0.5` or rise above `5.0` — outliers are
valid (e.g. `starwatcher_1.0` is balanced around 4.11) but should
be a conscious choice.

### 25. `LuaGaia/Gadgets/` needs a map-bundled `LuaGaia/main.lua` bootstrap

Shipping a gadget at `LuaGaia/Gadgets/Foo.lua` does nothing on its own.
The engine only scans that directory when the map carries a
`LuaGaia/main.lua` that `VFS.Include`s the engine-provided
`LuaGadgets/gadgets.lua` handler. `springcontent.sdz` (verified
against recoil 2026.06.04) provides the handler but **not** a
fallback bootstrap.

Real BAR maps (`gecko_isle_remake_v1.2.1`, …) ship the canonical
two-file pair:

```
LuaGaia/main.lua    -- synced bootstrap
LuaGaia/draw.lua    -- unsynced-draw bootstrap
```

`main.lua`:

```lua
if AllowUnsafeChanges then AllowUnsafeChanges("USE AT YOUR OWN PERIL") end
VFS.Include("LuaGadgets/gadgets.lua",nil, VFS.BASE)
```

`draw.lua`:

```lua
VFS.Include("LuaGadgets/gadgets.lua",nil, VFS.BASE)
```

**Rule:** `build_sd7` stages both files into every `.sd7`. They are
vendored at `crates/barme-pipeline/assets/luagaia_{main,draw}.lua`
and exposed as `featureplacer::LUAGAIA_{MAIN,DRAW}_SOURCE` (the
gadget infrastructure lives in `featureplacer.rs` as the principal
consumer). Pre-merge gate for any future "ship a gadget in
`LuaGaia/Gadgets/`" change: extract a real BAR map and diff our SD7
against it at the `LuaGaia/` level.

### 26. Don't ship empty `map_startboxes.lua` — and the shape is unwrapped

Two related findings from the 2026-05-19 smoke test:

**Existence beats content.** `luarules/gadgets/include/startbox_
utilities.lua::ParseBoxes:43` checks `VFS.FileExists("mapconfig/
map_startboxes.lua")` and uses the file's return value as-is. There
is no "is this table empty?" check. Shipping an empty file therefore
**suppresses BAR's default-fallback codepath** at lines 79–137 of the
same file (which would otherwise generate sensible N/S or E/W boxes
from map dimensions). An absent file is strictly better than an
empty one.

**Shape is unwrapped, in elmos.** Pre-Sprint-11 research had
`return { startboxes = { [0] = … } }`. That was wrong. Verified
against `titanduel_v3.sd7`'s `map_startboxes.lua` on 2026-05-19:
the file returns the per-ally-team table **directly**, with polygon
vertices in **elmo coordinates** (not 0..1 fractions):

```lua
return {
  [0] = {
    nameLong = "North-West", nameShort = "NW",
    boxes      = { { {0,0}, {614,0}, {614,614}, {0,614} } },
    startpoints = { {307, 307} },
  },
  [1] = { … },
}
```

The modoptions-string codepath at lines 56–59 *does* multiply
fractions by `Game.mapSizeX/Z`, but the map-file codepath does not.
Conflating those two formats yields silently-broken boxes.

**Rule:** `startboxes::should_emit(project)` is `true` only when the
project has ≥ 2 ally groups AND at least one has an authored
`box_polygon`. `build_sd7` checks this and skips staging the file
when `false`. When emitted, the file uses the unwrapped per-ally-
team shape with elmo-space polygons. `startpoints` carries one
entry at the polygon centroid in elmos.

### 23. Springboard featureplacer rotation is INTEGER, not string

A subtle within-`set.lua` schema detail: even though the C2 emitter
followed PyMapConv's `-k` flat-text convention (`rot = "0"` quoted)
through Sprint 11, the Lua-gadget consumer in
`LuaGaia/Gadgets/FP_featureplacer.lua` calls

```lua
Spring.CreateFeature(fDef.name, fDef.x, ..., fDef.z, fDef.rot)
```

`Spring.CreateFeature`'s rotation arg is numeric. A string rot
would coerce silently in some Lua versions but fail in others;
real BAR maps verified against
`gecko_isle_remake_v1.2.1.sd7`'s `set.lua` (`rot = -23991`)
consistently use the unquoted integer form. Different from PyMapConv's
`-k` flag — the two codepaths happen to share field names but the
rotation type differs.

**Rule:** `set.lua` rotation is an unquoted integer. PyMapConv's
text-file path (currently unused) uses the quoted string form per
the `-k` `--help` text. Don't conflate them.

## Sprint 14 follow-up additions (post-C9 smoke, 2026-05-19)

### 27. glam's `look_at_lh` flips the X axis sign vs RH conventions

A camera at `eye = (0, 0, +d)` looking at `(0, 0, 0)` along `-Z`,
with `up = +Y`, naively has its "right" axis pointing to world `+X`
(the user's right hand). glam's `Mat4::look_at_lh` builds the side
basis as `s = up.cross(forward)`. At the example above
`forward = (0, 0, -1)`, so `s = (0,1,0) × (0,0,-1) = (-1, 0, 0)`
— world `-X`. The resulting view matrix mirrors X relative to what
a RH-trained intuition expects: world `+X` (east) lands on the
LEFT side of the screen.

This bit the Sprint 14 arrow-key pan: the first pass derived the
screen-right axis from the RH formula `right_xz = (cos(yaw), 0,
-sin(yaw))`, which made ArrowLeft pan the user right and vice
versa. The empirically-correct axis for camera-relative panning
under glam's LH look-at is:

```
screen-right (world) = (-cos(yaw), 0,  sin(yaw))
screen-up    (world) = (-sin(yaw), 0, -cos(yaw))
```

**Rule:** when wiring keyboard or gamepad input to camera-relative
world motion under `glam::Mat4::look_at_lh`, flip the sign on the
"right" component of whatever cross-product formula you derived
from RH conventions. Verify empirically with a simple "press right,
camera should slide right under the user's view" test — math
alone is too easy to get wrong with mixed-handedness conventions.

### 28. BAR's water plane is `consteval`; users CANNOT move it

`RecoilEngine/rts/Map/Ground.h:23-38` makes
`GetWaterPlaneLevel()` a `consteval` returning `0.0f`. Water is
always at world `Y = 0`. The user can't raise or lower it.

Implication for the editor: a "water depth" or "water level"
slider would be a lie. The right affordance is a
`Project.min_height` control (negative = the lowest heightmap
sample sits below sea level → flooding is visible).

The first Tool::Water smoke (2026-05-19) caught a related defect:
the terrain shader's `sample_y` mapped raw `u16` linearly into
`[0, max_height]`, ignoring `Project.min_height` entirely. Even
with `min_height = -100` the heightmap rendered as if it started
at `Y = 0`, so the water plane sat flush with the floor and was
invisible. Fixed by extending the terrain Uniforms with
`params2.x = min_height` and updating `sample_y` to compute
`y = min_h + t * (max_h - min_h)`.

**Rule:** the terrain shader MUST consult `Project.min_height`
when projecting raw heightmap values to world Y, otherwise the
"flood a basin" workflow is broken even with the right data on
the project side. Pinned by the C9 inspector emission tests in
`barme-core::mapinfo_schema::tests` (which would catch a regression
on the data side) plus the shader uniform pin in `render.rs::tests`
(which catches a layout drift).
