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

**Rule:** `Project.geo_vents` emits ONLY into
`mapconfig/featureplacer/features.lua` with `name = "geovent"`. Never
write a `geos` array in `map_metal_layout.lua`.

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
