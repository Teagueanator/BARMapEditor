# Beyond All Reason GUI Map Editor — Research, Feasibility, SRS, and Stack

**TL;DR**
- BAR/Recoil maps are a tractable bundle of a binary SMF + tiled DXT1 SMT + a Lua manifest, glued together by one mature compiler (Beherith's PyMapConv). A standalone editor is feasible **only if** it treats PyMapConv as the build backend rather than re-implementing SMF/SMT compilation from scratch.
- The dominant hidden cost is not the GUI — it is the **texture pipeline at map scale**: a competitive 16×16 BAR map needs an 8192² diffuse + 8192² normal + 4096² splat distribution, all chunked into 32×32 DXT1 tiles. That dominates memory, disk, and compile time.
- Recommended stack: **Rust + egui/eframe + wgpu**, shipping a single static binary on Windows and Linux, with PyMapConv bundled as a sidecar. Tauri is the runner-up; Unity, Electron, and Java are rejected on packaging or performance grounds.

---

## Phase 1 — Research Findings

### 1.1 Engine context

Beyond All Reason runs on **Recoil**, a hard fork by the BAR team from the Spring RTS engine 105 tree (repo: `beyond-all-reason/RecoilEngine`, GPL-2.0; 552 stars / 220 forks as of May 2026 per the releases page; current release tag 2025.06.21). The Recoil wiki, last edited by maintainer `lhog` on 29 Mar 2024, states: *"Recoil RTS engine is a continuation and significant extension of the original Spring RTS engine version 105.0."* Practical implication: Spring-era map documentation is still the authoritative reference.

### 1.2 Map file anatomy

A BAR map ships as either an `.sd7` (7-Zip) or `.sdz` (Zip) archive containing:

```
mymap.sd7/
  maps/mymap.smf         -- binary: header + heightmap + tile indices + minimap + metalmap
  maps/mymap.smt         -- tile file: stream of 32×32 DXT1-compressed tiles
  maps/*.dds / *.png     -- normal, specular, splat distribution, DNTS detail, skybox, grass
  mapinfo.lua            -- single Lua table: atmosphere, lighting, water, splats, terrainTypes
  mapoptions.lua         -- optional player-tweakable knobs (e.g. WaterLevel presets)
  LuaGaia/, LuaRules/    -- optional gadgets (feature placement, custom rules)
```

**SMF binary** (from `spring/rts/Map/SMF/SMFFormat.h` plus the wiki SMF decompiler source): the header carries `mapx`, `mapy`, `squareSize=8`, `texelPerSquare=8`, `tileSize=32`, `minHeight`, `maxHeight`, then file offsets to heightmap (`short[(mapy+1)*(mapx+1)]`), typemap (`uint8[mapy/2 * mapx/2]`), tile-index data, minimap (always 1024×1024 DXT1 + 8 mipmap sublevels = exactly 699 048 bytes), metalmap (`uint8[mapx/2 * mapy/2]`), and an optional feature header.

**SMT tiles:** magic `"spring tilefile"`, version 1, tileSize 32, compressionType 1 (DXT1). The diffuse texture is sliced into 32×32 pixel tiles, deduplicated against a hash table, DXT1-compressed, and packed sequentially. The SMF references tiles by 4-byte indices into this pool.

**Image size rules** (per Spring `MakingMapsWithBluePrintAndMapConv` and the Zero-K mapping guide):

| Asset | Dimensions (Spring Map Units, where 1 SMU = 512 px texture = 64 px heightmap = 512 elmos world) |
|---|---|
| Texture map | `(512 × N)²`, must be multiples of 1024 |
| Heightmap | `(64 × N + 1)²`, 16-bit `.raw` or 16-bit `.png` preferred |
| Metal map | `(32 × N)²`, red channel = density |
| Type map | `(32 × N)²`, greyscale → terrain type index |
| Feature map | `(64 × N)²` (legacy pixel placement; superseded by Lua feature lists) |
| Grass map | `(16 × N)²` |
| Minimap | always 1024 × 1024 |

For a 16×16 BAR map: 8192² texture (~6 GB raw RGBA), 1025² heightmap, 512² metal/type, 256² grass. **Coordinates:** Spring uses a left-handed Y-up system; **8 elmos per heightmap texel**, **16 elmos per metal/type texel**, **1 elmo = 1 world unit**.

### 1.3 mapinfo.lua (BAR conventions)

Single returned Lua table with sections `smf`, `resources`, `splats`, `atmosphere`, `lighting`, `water`, `terrainTypes`, `grass`, `teams`, `custom`. BAR-typical resource entries are PBR-style: `detailTex`, `specularTex`, `splatDistrTex`, `splatDetailTex`, `splatDetailNormalTex1..4` (DNTS — Detail Normal Texture Splatting, in DDS), `detailNormalTex`, `skyReflectModTex`. `splats.texScales` and `texMults` tune each of 4 RGBA channels of the splat distribution map. `voidWater = true` combined with omitting `water.planeColor` produces the popular "space map" look (e.g. *Apophis*, *Quicksilver*). **`splatDetailNormalTex` requires a `specularTex` to be defined or it silently disables.**

**STATUS UPDATE 2026-05-17 (Stage 0 goal #7 findings):** the "required vs optional" calculus has three independent layers, NOT one — see `docs/PITFALLS.md` §"BAR Chobby + mod-gadget mapinfo expectations":

1. **Engine scanner**: only `name`, `smf.smtFileName0`, and `teams[*].startPos` are strictly required (per `burnhamrobertp/97cae4d300e675ca261e661fc58266d1` gist — the de-facto BAR map-format reference).
2. **Chobby map browser** (`beyond-all-reason/BYAR-Chobby` `LuaMenu/widgets/gui_maplist_panel.lua`): requires `modtype == 3` and the map must be in Chobby's "certified maps" list to appear in multiplayer lobbies. **Unofficial maps only appear in Skirmish / singleplayer.** The `GetMinimapImage not found` warning is benign — auto-extracted from SMF on first scan.
3. **BAR mod gadgets** (e.g. `luarules/gadgets/unit_sunfacing.lua`): read mapinfo subtables (`lighting`, `atmosphere`, `water`, …) directly without nil-checking. Missing subtables crash gadget load → game hangs at "waiting for players". The emitter MUST include conventional subtables with sensible defaults even though the engine itself would tolerate omission.

The `barme-pipeline::mapinfo` emitter's field set is calibrated to satisfy all three layers, and grows as new gadget nil-derefs are discovered. The list of subtables is not a Lua schema — it's a "minimum set BAR's mod gadgets won't crash on".

### 1.4 Existing toolchain (active vs. legacy)

| Tool | Stack | Status | Role |
|---|---|---|---|
| **PyMapConv** (Beherith / `springrts_smf_compiler`) | Python 3 + PyQt + nvdxt.exe | **Active, canonical.** Forum consensus 2021–2025: "deprecate all other mapconvs and make pymapconv 'the' mapconv." | SMF + SMT compile/decompile, GUI + CLI, ships a Windows one-file `.exe`; Linux runs from source — **STATUS UPDATE 2026-05-17 (ADR-011):** v0.6.3 ships a self-contained Linux ELF bundle (PyInstaller; ~90 MB extracted). No Python 3, PyQt, or Pillow install required on Linux. Upstream `--help` is broken in v0.6.3 (argparse crash); flag surface is captured in `devlog/stage-0-validation/logs/2026-05-17T16-57-48__pymapconv-vendoring.md`. **CORRECTION 2026-05-17 (ADR-014):** the bundled `tools/dragon-dxt1`, `tools/dragon-dxt5`, `tools/magick` are auxiliary GUI converters, *not* the `--linux`-mode compile path's dependency — that path shells out to `CompressonatorCLI` by name (upstream `src/pymapconv.py` lines 828 + 1032, no path override). Compressonator is therefore vendored separately under `tools/compressonator/` via `scripts/fetch-compressonator.sh`. The PyMapConv subprocess driver also passes `-q 1` to work around a v0.6.3 read-back bug (multi-thread tile path expected on Linux but tiles are written flat) and treats artifact-presence as the success contract (PyMapConv exits 1 on Linux even after a clean compile — bundled Qt event loop quirk). All recorded in `devlog/stage-0-validation/logs/2026-05-17T17-24-52__pymapconv-subprocess-driver.md`. |
| **SpringBoard** (gajop / `Spring-SpringBoard`) | Lua, runs *inside* Spring/Recoil | 0.5.3 announced by gajop on 23 Sep 2017, last forum activity 6 Dec 2018; BYAR variant exists but is inactive | Most feature-complete editor: heightmap raise/set/smooth, DNTS/specular/diffuse painting, void tool, feature & unit placement, undo/redo. *In-engine, not standalone.* |
| **SpringMapConvNG** (tizbac) | C++ + DevIL | Legacy (last meaningful work 2023) | Cross-platform CLI compiler; historical Win32 free() crash |
| **SpringMapEdit** (Heiko Schmitt → aeonios) | Java + SWT + JOGL | Abandoned (~2009–2012) | Standalone 3D editor: brushes, hydraulic/thermal erosion, auto-texture, mirror/flip/shift; no metal/feature/sd7 |
| **World Machine** | Commercial Windows app | Active | Procedural terrain + texture generator; Beherith ships a `.tmd` template for BAR. CPU/RAM intensive (16 GB RAM for a 16×16 final render) |
| **hendkai/bar-map-generator** | Web JS UI + Python | Early (2024–2025), self-described unfinished | Procedural generator that shells out to PyMapConv; not an editor |
| **tebeer/BARMapEdit** | Unity (C#) + Dear ImGui + custom HLSL | Personal/dormant: 22 commits, 0 stars, no LICENSE, no README, not on the official BAR mapmaking-resources page | Earliest-stage standalone GUI attempt. **Not a viable fork base.** |
| **Jandodev/bar-editor** | Vite + Vue 3 + TypeScript + Three.js (WebGL) | Early but usable (2025+); MIT | In-browser SPA editor. Reads `.smf` natively in TypeScript (no PyMapConv dep), terrain brushes (Add/Remove/Smooth + Flatten/Erode/Terrace), SMF save/export working; package export WIP. Polar-opposite architecture to ours (browser vs native single-binary); their TypeScript SMF parser is a useful reference for our Stage 1+ decompile-import. Found 2026-05-17 during goal #7 work. |
| Legacy MapConv variants (`spring/MapConv`, `pajohns/MapConv`, `enetheru/smf_tools`) | C++ | Legacy | Original CLI compilers requiring nvdxt.exe |
| **Feature Placer** | rapid tag `feature-placer:test` | Active | Spring-based 3D feature painter that exports `set.lua` |

**Beherith's recommended pipeline today** (per the *Advanced SpringRTS Mapping Guide* Google Doc and `beyondallreason.info/guide/mapmaking-resources`): World Machine (using his `.tmd` template) → PyMapConv → SpringBoard for DNTS painting and feature finetuning → 7-Zip into `.sd7`.

### 1.5 Distribution

BAR maps are curated through `github.com/beyond-all-reason/maps-metadata` (Apache-2.0, TypeScript; source-of-truth is `map_list.yaml`, generated from a Rowy table at `rowy.beyondallreason.dev`). Chobby (the lobby) auto-downloads via `pr-downloader`/rapid. Custom maps not in the curated list can simply be dropped into `Beyond-All-Reason/data/maps/`. There is no per-map review API; approval is human-mediated via Discord.

### 1.6 Planetary Annihilation reference UX

PA's in-game system designer is the cited gold standard. It does the following well, and the BAR editor should mirror:
- **Biome dropdown** (desert / earth / metal / ice / lava / asteroid / tropical) drives terrain + texture set in one click.
- Single **radius**, single **height-range**, and a **temperature** slider that re-colors the texture distribution rather than just toggling biomes.
- **Water-depth** slider with gameplay-aware semantics (deep = naval, shallow = hover/amphib).
- **Symmetry** toggles: terrain mirror, CSG mirror, metal/spawn mirror — non-negotiable for competitive maps.
- **Brush-based sculpting** (raise/lower/flatten/smooth) with seed-based regeneration so a "looks-bad" planet is one click away from rerolling.
- **Preview Terrain** vs. **Preview Gameplay** toggle — same camera, different overlay.

---

## Phase 2 — Feasibility Analysis (with Hidden Pitfalls)

**Verdict: feasible by one motivated CS student in 9–15 months to MVP, provided the editor delegates SMF/SMT compilation to PyMapConv as a bundled sidecar.** Re-implementing SMF/SMT compilation natively is a 3-month detour with negligible upside, and reintroduces the texture-dedupe + nvdxt headaches that PyMapConv has already solved.

### 2.1 Pitfalls that will actually hurt

1. **Texture pipeline memory.** A 16×16 map = 8192² diffuse. Holding it uncompressed (256 MB RGBA) + an 8192² normal (256 MB) + a 4096² splat distribution (64 MB) + an undo stack is trivially 2–4 GB resident. Use a **tiled copy-on-write 256×256 chunk model** with an LRU disk cache; never snapshot-undo a whole heightmap.
2. **DXT1 compression is slow and lossy.** PyMapConv invokes `nvdxt.exe` (NVIDIA's legacy DXT compressor, Windows binary; runs under Wine on Linux). Quality-tuned compression of a 16×16 takes 1–10 minutes. Use **bc1 (texpresso, bcdec/bcenc, or ISPC Texture Compressor) in-process for live preview**, fall back to nvdxt for final-quality `.smt`. The SMT format mandates DXT1 specifically (`compressionType=1, tileSize=32`); BC7 is not an option. — **STATUS UPDATE 2026-05-17 (ADR-004):** upstream PyMapConv now uses AMD Compressonator (native Linux binary, open-source) in place of `nvdxt.exe`. No Wine dependency on Linux. Live-preview BC1 still warranted for sub-second feedback, but the "fall back to nvdxt" leg of this pitfall collapses.
3. **Tile deduplication.** The SMT format hash-deduplicates 32×32 tiles. Naïve output produces SMTs roughly 4× larger than tuned output. PyMapConv has the deduplicator; if you ever fork it, port the hash table verbatim.
4. **Heightmap edge constraint.** Must be exactly `(64·N + 1)²` — **not** a power of two. Crop/pad logic for image import is the #1 silent failure mode (mapconv warns + resizes, producing visibly wrong terrain).
5. **Coordinate sign flips.** Spring is Y-up, left-handed. Heightmap pixel `(x, y)` corresponds to world `(x·8, height, y·8)`. The `-i / --invert` mapconv flag exists because of historical row-order confusion. Lua features use `{x, z, rot}` in world elmos. Pick one convention internally and bake it in.
6. **mapinfo.lua silent dependencies.** `splatDetailNormalTex` requires `specularTex`; `voidWater` requires unsetting `water.planeColor`; missing or renamed `smtFileName0` produces the **infamous pink map**; `fogStart == fogEnd` (e.g. both 1.0) breaks the ground-grid renderer. The editor must run a linter pass before save.
7. **Pink-map trap on rename.** Historically the SMT filename was hardcoded into the SMF; modern Recoil allows override via `mapinfo.smf.smtFileName0`. The editor must rewrite mapinfo whenever the SMT is renamed.
8. **DNTS + water + LOS bug** (Beherith, springrts forum t=35202): with `minHeight < 0` plus DNTS plus a Lua widget that touches LOS, you get animated TV-snow artifacts. Warn when DNTS is enabled on a water map.
9. **`.sd7` solidity.** 7-Zip solid archives are silently rejected by SpringFiles indexing. The packager must emit **non-solid** archives.
10. **License of the output stack.** Recoil is GPL-2.0; legacy mapconv binaries are GPL-2.0; **PyMapConv has no SPDX-declared license**. Redistributing PyMapConv inside your installer requires explicit written permission from Beherith. This is a hard prerequisite. — **STATUS UPDATE 2026-05-17 (ADR-003):** upstream now carries an SPDX `CC0-1.0` LICENSE file. Redistribution is unrestricted; the "ask Beherith for written permission" workstream is removed (we still credit him in `CREDITS.md` out of courtesy).
11. **3D preview ≠ in-game rendering.** Recoil's actual ground shader (DNTS + splats + PBR + atmospheric scatter + dynamic shadows) is non-trivial; the editor preview will be an approximation. Document this up front; do not pretend WYSIWYG.
12. **Decompilation fidelity.** Round-tripping an existing `.sd7` loses information: the recovered diffuse PNG has been through DXT1 (color precision loss); heightmap, metal, and type maps are exact; mapinfo.lua is exact; auxiliary splat textures survive untouched. Reuse PyMapConv's decompile path.
13. **GPU brush latency.** Spring/Recoil maps can theoretically reach 96×96 SMUs. Sub-millisecond brush response at 32×32+ requires the heightmap to live on the GPU as a storage texture, edited by compute shaders. Read-back to CPU happens only at save.

### 2.2 Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| PyMapConv breaks on a new Recoil release | M | H | Vendor a pinned PyMapConv build; CI test against the latest Recoil release tag |
| nvdxt.exe unavailable on Linux native / ARM | H | M | Bundle a native BC1 encoder (texpresso / bcdec) for in-tool preview & builds; keep nvdxt only for final-quality compile — **STATUS 2026-05-17 (ADR-004):** PyMapConv now uses AMD Compressonator (native Linux); risk collapses to L/L.** |
| Beherith refuses redistribution of PyMapConv | L | H | Fallback: download PyMapConv at first launch (the Springboard model) |
| Memory blow-out at 32×32+ map sizes | M | H | Tiled COW edit buffer, disk-backed undo |
| Editor outputs invalid mapinfo.lua | H | M | Schema validator + headless test using Recoil `--isolation` pre-release |
| Scope creep into a generic Spring editor | H | M | Lock to BAR conventions in MVP; expose Spring-genericity as v2 |

---

## Phase 3 — SRS / SRD

### 3.1 Vision

A single-window, single-executable desktop app that produces a *playable* BAR map from an empty project to a tested `.sd7`, on both Windows and Linux, with the UX feel of Planetary Annihilation's system designer.

### 3.2 Functional requirements

| # | Requirement | MVP | v1 | v2 |
|---|---|---|---|---|
| F1 | New-project wizard: size (units), biome preset, symmetry mode | ✅ | | |
| F2 | Real-time heightmap sculpting: raise/lower/flatten/smooth/erode/ramp | ✅ | | |
| F3 | Symmetry enforcement (mirror H/V/diag/rot-N) on heightmap and overlays | ✅ | | |
| F4 | Texture painting via DNTS splat channels (4 RGBA, ≤4 splat textures) | ✅ | | |
| F5 | Metal-spot placement (point + radius → red-channel density on metal map) | ✅ | | |
| F6 | Geo-vent placement | ✅ | | |
| F7 | Feature placement (trees, rocks, wreckage) with rotation/scale, into a Lua feature gadget | ✅ | | |
| F8 | Start-position editor | ✅ | | |
| F9 | mapinfo.lua editor (form view + raw Lua tab) | ✅ | | |
| F10 | Minimap auto-generation from texture + manual override | ✅ | | |
| F11 | One-click `.sd7` build via bundled PyMapConv | ✅ | | |
| F12 | "Launch in BAR" button (invokes Recoil with the test map) | ✅ | | |
| F13 | Decompile/import existing `.sd7` | | ✅ | |
| F14 | Procedural terrain generation (FBM, hydraulic erosion, river carve) | | ✅ | |
| F15 | Type-map editor + per-terraintype gameplay params | | ✅ | |
| F16 | Skybox picker / atmospheric preset library | | ✅ | |
| F17 | Pathability overlay (which units can traverse where) | | ✅ | |
| F18 | Heightmap import from real DEM (GeoTIFF) | | | ✅ |
| F19 | Procedural feature scatter with rule sets | | | ✅ |
| F20 | "Publish to BAR" — open a PR against `maps-metadata` with generated YAML row | | | ✅ |
| F21 | Light/dark theme toggle (egui theme, persisted across launches) | | ✅ | |
| F22 | Bottom status bar: live CPU% + resident memory of the editor process | | ✅ | |
| F23 | User-asset library: importable terrain stamps + feature prefabs (PA-style "drop a bridge / mountain here") | | | ✅ |

> **STATUS UPDATE 2026-05-17 (user request):** F21/F22 added after Stage 0
> goal #7. F21 is straight egui (`ctx.set_visuals(Visuals::dark/light())`)
> with a `serde`-persisted preference. F22 needs a per-platform process-stats
> probe (`sysinfo` crate is the obvious choice — pure-Rust, cross-platform,
> already used elsewhere in the wgpu/Rerun ecosystem). Refresh once per
> second, render in an egui `TopBottomPanel::bottom` so it's always visible.
> Memory should be RSS in MiB; CPU is process-local %, smoothed over the
> sample window. Out of scope for Stage 0.

> **STATUS UPDATE 2026-05-17 (user request, F23):** Planetary Annihilation's
> system designer lets authors drag user-uploaded planetary set pieces
> (mountains, biomes, structures) onto the map. The BAR-equivalent splits
> into three orthogonal asset types, each with its own bundling
> implications:
>
> 1. **Heightmap stamps** — small 16-bit PNG patches the user paints into
>    the project's main heightmap. Pure CPU/GPU operation; no `.sd7`
>    payload impact. Cheapest to ship.
> 2. **Feature prefabs (trees, rocks, wreckage, bridges)** — these are
>    3DO / S3O / OBJ models that BAR's mod gadgets place via
>    `LuaGaia/featuredefs.lua` + a placement table. **Default features
>    (trees, generic rocks) are owned by the BAR mod and referenced by
>    name** — zero `.sd7` payload, but the user's choices are limited to
>    what the mod ships. **Map-custom features** would need their model
>    + texture files bundled into the `.sd7`, which inflates the archive
>    fast (a single S3O bridge with diffuse/normal/specular at 1024² is
>    ~3 MB). The library should distinguish "mod-provided" (free,
>    portable) from "map-bundled" (heavy, locks the user into shipping).
> 3. **Splat / DNTS material packs** — DDS-compressed splat textures the
>    user drops in as DNTS layers. Heaviest individually
>    (256–512 KB per BC1 splat at 1024²) but reused across the splat
>    distribution map, so the marginal cost is bounded.
>
> Architectural note for whoever scopes this: the library belongs in
> `barme-core` as a registry layer (asset metadata + on-disk paths), and
> `barme-pipeline` is responsible for resolving "mod-provided" references
> at build time (refuse to bundle, warn if the named feature isn't in
> the BAR mod's default set) and bundling "map-bundled" assets into the
> staging tree before 7-Zip. **Don't bake the asset library into the UI
> shell** — both a "Browse community assets" panel and a CLI batch
> stamper should be able to consume it.
>
> Reference: PA's system designer (`uberent/PlanetaryAnnihilation`) and
> Spring's longtime feature-placement convention as documented in
> Beherith's *Advanced SpringRTS Mapping Guide*. Implementation gated
> on a v2 scope discussion — the file-format choices alone (do we share
> a `.barme-assetpack` tarball convention? piggyback on `.sd7`?) need an
> ADR before any code.

> **STATUS UPDATE 2026-05-17 (Stage 1 opener, F2):** Raise / Lower / Smooth
> shipped via the `barme_core::brushes` plugin-shaped trait + registry
> (ADR-018). New brushes (flatten / erode / noise / terrace / ramp) plug in
> as `impl Brush` + one line in `BrushRegistry::default_set` — no UI or
> dispatch edits. Kernels are CPU; bench at 16 SMU shows ~10× headroom
> against the NFR-Performance budget, so GPU compute port is formally
> deferred (ADR-021).

> **STATUS UPDATE 2026-05-17 (Stage 1 opener, F3):** Shipped via
> `barme_core::symmetry::SymmetryAxis` (ADR-019). Covers `None`,
> horizontal / vertical mirror, both (Quad), both diagonals, and
> rotational with a user-editable fold value (`Rotational { fold: 2..=12 }`
> via the side-panel DragValue — 3 for three-player maps, 4 for
> quad-player, etc.). One brush stamp produces N derived stamps; their
> dirty rects union into a single GPU upload. Arbitrary-axis line picker
> is Stage 2.

> **STATUS UPDATE 2026-05-17 (Stage 1 opener, F14 partial):** The
> math-function subset shipped via `barme_core::procgen::generate`
> (ADR-020). User enters `f(x, z) ∈ [0,1]` and the heightmap is
> regenerated at the project's MapSize. Powered by `evalexpr` v13;
> ships with seven presets (flat / parabolic bowl / dome / cone /
> ridge / ramp / sine ripples). FBM, hydraulic erosion, and river-carve
> remain Stage 2 — they need their own ADR (noise basis function,
> erosion solver choice, river network seeding).

> **STATUS UPDATE 2026-05-17 (F8 — shipped):** Phase 2 ADR-023 lands a
> 2D start-position editor in the central preview rect: LMB places /
> drags, RMB deletes. Symmetry from the F3 system replicates the stamp
> through mirror counterparts; team ids are assigned alternating
> even/odd via `barme-core::start_pos::assign_team_ids` (matches BAR's
> per-side `teams[]` convention). `barme_pipeline::mapinfo` emits
> authored teams when present, falling back to the 25/75 default pair
> when the vector is empty. `Project.start_positions` round-trips via
> `serde(default)`. Multi-position bulk operations + symmetry-grouped
> drag remain Phase-3 polish; this commit closes the editor surface F8
> implies.

> **STATUS UPDATE 2026-05-18 (F8 — allyteam tree, ADR-032 / B6):** the
> flat `start_positions` shape was wrong for 8v8 / 3-way FFA / 4-way
> FFA — those need an ally-team layer. `Project.start_positions`
> replaced by `Project.ally_groups: Vec<AllyGroup>` (id + name + sRGB
> colour + sources + optional `box_polygon`). Inspector becomes a
> tree with a configuration-preset dropdown (`1v1` / `8v8` /
> `3-way FFA` / `4-way FFA`), per-group collapsing header, and child
> rows for sources + greyed mirror entries. Canvas grows LMB-drag-
> paint: drag distributes N evenly-spaced positions along the vector
> (default 8). Hover↔pulse links the tree to the canvas (Inspector
> row → marker pulse; marker hover → tree scrolls). Markers ghost to
> 50 % alpha outside the StartPositions tool (B1 pattern). Symmetry
> mirrors land in the SAME ally group as the source (sources stored;
> mirrors recomputed each frame). Build path expands sources through
> the active symmetry before emission so the `.sd7` carries every
> spawn the user saw on canvas. Pre-Phase-3 `.barmeproj` migration is
> backwards-compatible: legacy `[[start_positions]]` materialises
> into `ally_groups[0]` via a custom `Deserialize`. ADR-023's data
> shape is superseded; its UX surface (LMB place / drag / RMB delete)
> survives.

> **STATUS UPDATE 2026-05-17 (F1 — shipped):** Phase 2 ADR-024 lands a
> modal new-project wizard as the app's entry point — auto-opens on
> launch, re-opens via File → New project. Fields: project name
> (sanitised via `sanitize_name`), rectangular `smu_x × smu_z` (2..=64
> each), symmetry preset (incl. user-editable rotational fold), biome
> preset (4 presets from `procgen::BIOMES`, each with a sensible
> `max_height_hint`), max height (64..=4096 elmos). Wires symmetry +
> max-height + biome procgen on Create. Existing in-memory "untitled
> 16×16" auto-start is gone; Cancel restores it as the default. App's
> `map_size_smu: u32` widened to `map_size: MapSize` along the way so
> rectangular survives outside the wizard too.

> **STATUS UPDATE 2026-05-17 (Project model — start_positions shipped):**
> `Project` now carries `start_positions: Vec<StartPosition>` (F8 /
> ADR-023) with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`
> so pre-F8 `.barmeproj` files load forward. Remaining Phase-3+ slots
> (`metal_spots`, `geo_vents`, `features`, `splat_distribution`,
> `mapinfo_overrides`) follow the same pattern when each F-feature
> lands.

> **STATUS UPDATE 2026-05-18 (Project model — ally_groups, ADR-032):**
> `Project.start_positions` superseded by `Project.ally_groups:
> Vec<AllyGroup>`. Each group carries id + name + sRGB colour +
> sources + optional `box_polygon` (0..1 fractions feeding
> `mapconfig/map_startboxes.lua` per ADR-029). `StartPosition` is now
> just `(x_elmo: i32, z_elmo: i32)`; team ids are positional at
> emission time. Custom `Deserialize` (via `#[serde(from =
> "ProjectWire")]`) migrates pre-Phase-3 flat `[[start_positions]]`
> into `ally_groups[0]` on load. `team_id` in legacy files is
> ignored via `serde::de::IgnoredAny`.

> **STATUS UPDATE 2026-05-18 (F9 — schema shipped; form editor pending):**
> Sprint 4 / C1 / ADR-028 lands the typed `mapinfo.lua` schema at
> `crates/barme-core/src/mapinfo_schema.rs` (`MapInfo` + 9 sub-blocks).
> `MapInfo::bar_default()` populates BAR conventions (modtype 3,
> gravity 130, extractor_radius 80, depend `["Map Helper v1"]`,
> atmosphere fog_start 0.1 / fog_end 1.0, splats tex_scales
> `[0.02; 4]` / tex_mults `[1.0; 4]`, lighting sun_dir as
> `[f32; 4]`). `impl From<&Project> for MapInfo` produces a schema
> instance from the current project, ready to feed the Lua emitter.
> `Project` gained `mapinfo_overrides: HashMap<String, toml::Value>`
> for F9's eventual form-edit bag. **F9 itself (form editor + raw Lua
> tab) is still pending — C7 will wire the UI on top of this schema.**

> **STATUS UPDATE 2026-05-18 (F9 — full BAR-default block, C3):**
> Sprint 6 / C3 fleshes out `MapInfo::bar_default()` with the digest's
> full BAR convention values: lighting colours and shadow densities
> per `Rendering/Env/SunLighting.cpp` defaults, atmosphere wind range
> (5..25) + fog colour + sky colour + sky direction + cloud density,
> and a four-entry `terrain_types` array (Default / Rock / Sand /
> Water) with the canonical `hardness` / `receiveTracks` / `moveSpeeds`
> values. New `MapInfo::bar_default_with_water()` constructor returns
> a struct with the `water` sub-table populated (surface / plane /
> min / base colours, fresnel, perlin params) for projects that opt
> in via `tidal_strength > 0` or `min_height < 0`. The Lua emitter
> already wrote `sunDir` (camelCase) — a regression test now also
> asserts the lowercase `sundir` form does NOT leak out. 11 schema
> tests + 3 emitter tests pin every named default; the smoke test
> checks `gravity == 130`, `extractor_radius == 80`, `modtype == 3`,
> `fog_start != fog_end`, `splats.tex_scales == [0.02; 4]`,
> `terrain_types.len() == 4`.

> **STATUS UPDATE 2026-05-18 (BAR source audit, mapinfo + splat
> corrections):** Direct read of `RecoilEngine` + `Beyond-All-Reason`
> + `BYAR-Chobby` clones at HEAD on 2026-05-18 surfaced ten
> load-bearing pitfalls and dozens of field-level corrections vs the
> existing research digest. Full write-up at
> `docs/research/source-audit-2026-05-18/FINDINGS.md`. Highlights
> that change emitter behaviour:
>
> 1. **`lighting.sundir` vs `lighting.sunDir`** — engine reads
>    camelCase only (`MapInfo.cpp:207`); BAR's active
>    `unit_sunfacing.lua` (March 2024) reads lowercase only.
>    Lua tables are case-sensitive — **emitter MUST write BOTH
>    keys** with the same value. The "regression test asserts
>    sundir does NOT leak out" claim in the C3 status above is
>    now incorrect; the test must be inverted to require both
>    keys present in the rendered output. Sprint 6 follow-up
>    item; tracked in the FINDINGS doc.
> 2. **`atmosphere.skyDir` is deprecated** (engine logs
>    `L_DEPRECATED`); use `atmosphere.skyAxisAngle` (float4: xyz
>    rotation axis + radians angle). C3 must rename the field
>    in its `bar_default()` block.
> 3. **Geo vents are NOT a `geos` array in `map_metal_layout.lua`**
>    — that's Zero-K convention. BAR derives geo positions from
>    features with `geoThermal = true`. F6 emits `geovent`
>    feature placements only; the metal-layout file carries
>    only `spots = {...}`. The C2 emitter scaffolding is correct
>    on this point.
> 4. **SMF metalmap MUST be all-zero when emitting Lua spots** —
>    `map_metal_spot_placer.lua` (BAR gadget, raaar 2017) bails
>    if any metalmap pixel is non-zero. F5 must ship a black
>    metalmap PNG to PyMapConv at build time.
> 5. **`lighting.sunDir.w = 1.0` by default**, not `1e9` as the
>    earlier research digest stated. Engine code:
>    `float4(0.0f, 1.0f, 2.0f, 1.0f)` (`MapInfo.cpp:207`).
> 6. **`splatDetailNormalTex` subtable form** —
>    `resources.splatDetailNormalTex = { "a.dds", "b.dds", ...,
>    alpha = true }` is the modern path; the emitter should
>    prefer it over the legacy `splatDetailNormalTex1..4` keys.
> 7. **`modtype` enum** has six values: 0=hidden, 1=primary,
>    2=unused, 3=map, 4=base, 5=menu (per `ArchiveScanner.cpp:83`).
> 8. **`gui.minimapRotation` is unused** by current Recoil — engine
>    reads only `gui.autoShowMetal`. Drop from the C3 emitter
>    defaults if present.
> 9. **`map.voidAlphaMin` exists** (default 0.9, voidGround
>    threshold) and is missing from the current research's field
>    table. Add to F9 schema.
> 10. **PyMapConv does NOT touch `mapinfo.lua`** — it emits SMF +
>     SMT + a `_featureplacement.lua` baked into the SMF feature
>     header. All `mapinfo.lua` + Lua-sidecar emission is the
>     editor's responsibility.
>
> The splat-rendering ADR (anticipated ADR-035 / F4) also picks up
> five corrections — most importantly the tangent basis is built
> from the per-fragment normal (not a static `T=+X / B=+Z`), the
> base normal decodes from `normalsTex.ra` channels only with Y
> derived, and the specular exponent is `α × 16` (flat), not a
> `mix`. See FINDINGS §7 for the full corrected math. The current
> proposed WGSL in `splat-rendering/claude findings.md` will
> render visibly wrong if implemented verbatim.

> **STATUS UPDATE 2026-05-18 (F14 — procgen UX redesign, B7):**
> Sprint 6 / B7 reorders the Procgen Inspector to preset-first per
> the UI research digests: preset dropdown (auto-detects "Custom"
> when the expression diverges) → `CollapsingHeader` "Custom
> expression" (collapsed by default) → domain radio → 256² greyscale
> preview thumbnail → "Apply to heightmap" button + ✓ / ✗ chip.
> Preview backs to a persistent `egui::TextureHandle` reused via
> `handle.set(image, options)` so the GPU page count stays flat
> across keystrokes. New `barme_core::procgen::generate_thumbnail`
> helper reuses A3's `PixelContext` + precomputed norms; 256²
> cone-peak lands ~25 ms in release. A 50 ms debounce keyed on
> `hash(expr, domain)` coalesces multi-keystroke bursts and flips
> the cache even when the expression string is unchanged but the
> domain toggles. `ctx.request_repaint_after(remaining)` wakes the
> UI loop on quiescence so the preview catches up without busy-
> spinning.

> **STATUS UPDATE 2026-05-18 (F1 — demo state on Create, B8):**
> Sprint 6 / B8 turns the wizard's Create button into a true
> "ready to play" handoff. `apply_wizard` now: (1) seeds two start
> positions in `ally_groups[0]` on N / S strips (15 % / 85 % of map
> Z) — runs **after** the biome procgen so a valley-finder can
> dodge the parabolic-dome's peak, falling back to the map
> quarter-points on a miss; (2) reframes the camera at 35° pitch /
> 1.6 × diagonal distance from map centre; (3) opens a non-modal
> `egui::Window` overlay with three accelerator-named bullets
> (B / S / G) pointing at the demo interactions. Wizard's default
> symmetry flipped from `None` → `Horizontal` so the markers line
> up with the symmetry axis without further input. Dismiss
> persists per-project via a new `Project.next_steps_dismissed`
> bool (default false, `skip_serializing_if = "std::ops::Not::not"`);
> a fresh wizard Create on a new project re-shows the hint, by
> design — the user should not have to opt-back-in across
> unrelated projects.

> **STATUS UPDATE 2026-05-18 (F11 — Lua AST emitter + three-file
> convention, ADR-029 / C2):** the ad-hoc string formatter at
> `barme-pipeline::mapinfo` is gone. New `barme-pipeline::lua_ast`
> exposes `LuaKey`/`LuaValue` + a 2-space-indent, alpha-sorted,
> trailing-comma renderer. The mapinfo emitter walks the typed
> [`MapInfo`] schema from C1; the BUILD path stages **four** Lua
> files into the `.sd7` (was: one):
> - `mapinfo.lua` (root)
> - `mapconfig/map_metal_layout.lua` (empty placeholder; C4 / C5
>   populate spots + geos)
> - `mapconfig/map_startboxes.lua` (populated from
>   `Project.ally_groups[*].box_polygon` per B6; empty when
>   `ally_groups.len() <= 1`)
> - `mapconfig/featureplacer/features.lua` (empty placeholder;
>   C6 populates)
>
> The renderer is deterministic — repeated builds produce
> byte-identical `.sd7` (NFR-Det), pinned by
> `determinism_repeated_render_byte_identical` in each emitter
> module. `description` escapes `\`, `"`, `\n`, `\r`, `\t`
> correctly (round-trip pinned). ADR-013's emitter half is
> superseded; the packaging half (`7z -ms=off`, post-build
> `Solid = -` check, PITFALL #7 defence) remains in force.

> **STATUS UPDATE 2026-05-18 (Undo for non-heightmap state — B5):**
> Sprint 4 / B5 lands a unified
> `enum HistoryEntry { Heightmap(HeightmapEntry), Project(ProjectDiff) }`
> stack so F8 start-position edits and F1 wizard applies are
> undoable on the same Ctrl-Z that walks back brush strokes.
> `ProjectDiff` covers `PlaceStartPosition`, `MoveStartPosition`,
> `DeleteStartPosition`, `ApplyWizard(WizardSnapshot)`. Eviction is
> largest-first across both channels so a single long stroke can't
> evict 20 recent F8 placements. Undo/redo gated on
> `!is_dragging_anything()` so the user can't peel back state
> mid-gesture. No new ADR — extends ADR-033's per-stroke COW.

> **STATUS UPDATE 2026-05-18 (Canvas affordances — Sprint 3 / B2 B3 B4):**
> The B1 layout shell now has canvas-side feedback for every editor
> session:
> - **B2 / ADR-031** — Symmetry overlay paints dashed mirror axes /
>   rotational spokes through the map centre whenever
>   `symmetry != None`. Sculpt + mirror mode renders mirror-brush
>   ghost rings at every symmetry-derived centre (50% alpha) so the
>   user can predict where the stroke will land. F3 surface.
> - **B3** — Primary brush ring at the cursor (Raise green / Lower
>   red / Smooth blue), nav-gizmo top-right of the viewport (6 axis
>   snaps + drag-orbit), first-launch hint Window persisted via a
>   new `EditorConfig` (TOML at the OS config dir, version-keyed
>   so a major release replays once), and a `?` cheat-sheet modal
>   auto-generated from `Tool::ALL` + a `CAMERA_BINDINGS` table.
>   F2 / F3 / F14 surface; new module `crates/barme-app/src/config.rs`.
> - **B4** — Build & Install promoted from a plain side-panel
>   button to a primary-coloured top-bar Button + `ComboBox` variant
>   selector. Button fill comes from `Visuals::widgets::active.bg_fill`
>   so the future F21 theme toggle stays correct. Bottom status
>   strip now requests a 1 Hz repaint so camera coords keep ticking
>   when nothing else is animating. F11 surface; F12 Launch variant
>   permanently greyed until F12 ships.
> Test counts: `barme-app` 18 → 112 (94 new across overlay / gizmo
> / cheat_sheet / intro / config / app-level). `barme-core` /
> `barme-pipeline` unchanged. No NFR regressions; no new ADRs
> beyond ADR-031.

> **STATUS UPDATE 2026-05-18 (Editor layout shell — ADR-030, F2/F3/F8/F14
> UI surfaces re-homed):** The pre-Phase-3 stacked-side-panel UI is gone.
> The editor now uses a five-zone shell: top action bar (File/Edit/Build
> menus + symmetry chip + Build & Install), bottom status strip (camera
> readout + map dims + validation-chip placeholder), left 40 px tool
> strip (Q Select / B Sculpt / S StartPositions / G Procgen), right
> 300 px resizable Inspector (persistent project header + tool-specific
> controls via exhaustive `match`), central wgpu viewport (panel
> add-order LAST). Tool-specific state stays on `App`, not
> `ui.memory()`. Single-active-tool `Tool` enum has room for Phase 4
> Splat / Metal / Feature variants without dispatch-site changes — the
> exhaustive match enforces handling. Drag threshold raised to 8 px
> (`InputOptions::max_click_dist`) to disambiguate click-place from
> drag-paint. Symmetry promotion from Sculpt-only radio to top-bar chip
> + popover preserves the existing controls (B2 / ADR-031 adds the
> canvas overlay). All Phase-2 features still reachable: Ctrl-Z undo,
> F8 placement, F1 wizard via File → New, Procgen Apply, symmetry
> mirror replication. 8 new unit tests pin the Tool enum and set_tool
> invariants; 3 more pin Phase 2 smoke paths
> (`b1_does_not_regress_*`).

> **STATUS UPDATE 2026-05-19 (UI visual overhaul — ADR-035, design-mockup
> adoption):** Within the same five-zone shell, the editor's visual
> identity, widget library, and per-panel content were overhauled to
> match the Claude Design mockup at `docs/research/ui/`. **Structural
> layout is unchanged** — panel add-order, the exhaustive-`match` tool
> dispatch, and ADR-030's invariants all still hold. What did change:
>
> - **Theme + widget library:** new `crates/barme-app/src/ui/theme.rs`
>   (single `Tokens::DARK` palette — bg / panel / hover / border /
>   accent / chip-tone colours) and `crates/barme-app/src/ui/widgets.rs`
>   (`section`, `chip`, `ramp_slider_labelled`, `pill_toggle`,
>   `split_button`, `key_combo`, `icon_button`). All panels now read
>   colours from one source.
> - **Icon set:** new `crates/barme-app/src/ui/icons.rs` paints ~42
>   Lucide/Tabler-style line icons directly via `egui::Painter`
>   (no font dependency). Replaces Unicode glyphs in the tool strip,
>   top bar, viewport chrome, and modals.
> - **Tool strip extension:** `Tool::SplatPaint` (T), `Tool::MetalSpots`
>   (M), `Tool::GeoFeatures` (F) added alongside Select / Sculpt /
>   StartPositions / Procgen. Tile renders 36 × 36 line-icon + letter
>   underneath; active state = filled accent bg + 2 px left rail.
> - **Top action bar:** brand chip, File / Edit / **View** (new — grid /
>   lighting / wireframe overlay toggles) / Build menus, **centred
>   symmetry cluster** (pill toggle + mode dropdown + fold spinner —
>   replaces the popover-only widget), right-aligned **validation chip**
>   wired to `App::validation_summary()`, **Save button with dirty dot**,
>   and **split-button Build & install** with caret menu for variants.
> - **Inspector visual update:** project header now has an editable name
>   field + size `DragValue`s + saved/unsaved chip; heightmap card is a
>   2-col grid with a valid/invalid chip. Sculpt panel uses a 4-card
>   brush picker (Off / Raise / Lower / Smooth) with colour-coded
>   swatch rings, ramp-slider radius/strength, falloff preview, and a
>   behaviour chip row (Continuous active; Pressure / Lock-Z reserved).
>   Procgen panel surfaces presets as chips, syntax-highlights expression
>   errors with a red wavy outline + error tooltip, and disables Commit
>   while invalid. Start-positions panel groups controls into a Layout
>   section (preset dropdown + drag-paint count + Balanced/Asymmetric
>   chip) and an Allyteams section with collapsible cards.
> - **Viewport chrome (replaces XYZ nav gizmo):**
>   `crates/barme-app/src/ui/gizmo.rs` is **deleted**. New
>   `crates/barme-app/src/ui/minimap.rs` paints a **top-down mini-map**
>   in the top-right (heightfield biome-ramp thumbnail, symmetry
>   bisector, allyteam pins, metal spots, camera frustum, tiny N-arrow
>   compass). New `crates/barme-app/src/ui/viewport_chrome.rs` adds
>   left + bottom elmo rulers, a floating top-left viewport-options
>   toolbar (grid / lighting / wireframe + view-mode chip), a
>   bottom-centre first-launch hint card, and an "empty-state · Create
>   map" CTA when no heightmap is loaded.
> - **New project wizard + cheat sheet:** wizard restyled as a split
>   layout (name / size / max-height on the left; symmetry + biome
>   preset cards on the right) with a footer info chip and Cancel /
>   Create buttons. Cheat sheet restyled as a 2 × 2 group grid (Camera /
>   Tools / Sculpt / Project) using `key_combo` chips.
> - **Scaffolding for Phase 4 streams:** `inspector_splat`,
>   `inspector_metal`, and `inspector_geo` now render the full mockup
>   layout but back onto in-memory state (`App::splat_state` /
>   `metal_state` / `geo_state`). The F-series schema work (F4 splat,
>   F5 metal, F7 features) will swap that state for `Project`-persisted
>   schemas without changing the visual surface.
>
> Test count after the overhaul: **318 passing** (149 barme-app,
> 117 barme-core, 52 barme-pipeline). New tests cover theme palette
> distinctness, icon catalogue uniqueness, ramp-slider value math,
> mini-map projection, biome ramp monotonicity, ruler tick count,
> validation-summary tones, `start_positions_balanced` semantics, the
> symmetry pill-toggle round-trip, and the Phase-7 default scaffolding
> state. The deleted XYZ-gizmo's ~20 tests were retired in lockstep.

### 3.3 Non-functional requirements

- **NFR-Performance:** Brush stroke latency ≤ 8 ms on a 16×16 map at 60 fps preview on a mid-range 2020 GPU.
- **NFR-Memory:** Resident set ≤ 4 GB editing a 16×16 map; ≤ 8 GB at 32×32.
  - **STATUS UPDATE 2026-05-18 (A1 / ADR-033):** undo history now obeys
    the 100 MB ring cap reliably. The prior per-stamp snapshot model
    (ADR-022) blew past that cap by 2-3× on long brush strokes —
    single stroke at radius 1024 captured ~244 MB on a 16-SMU map.
    Copy-on-first-write within a stroke bounds a single `UndoEntry` at
    `bbox.w × bbox.h × 2 bytes` (≤ ~2 MB at 16 SMU, ≤ ~9 MB at 32
    SMU), independent of stamp count.
- **NFR-Portability:** Single static binary on Windows x86_64 and Linux x86_64; AppImage for Linux. No system-wide install required.
- **NFR-Toolchain:** Bundle PyMapConv + Compressonator under a `tools/` subdirectory of the executable.
  Also requires the host system to provide a 7-Zip binary (`7z` / `7zz` / `7za`) — declared in install docs, not bundled.
- **NFR-Determinism:** Same project → byte-identical `.sd7` (compile timestamps stripped).
- **NFR-Crash safety:** Autosave every 60 s to disk-backed project file.
- **NFR-Audit:** Emitted mapinfo.lua is human-readable, diff-friendly, and matches BAR community style.
- **NFR-Observability:** All operations are traced via `tracing` with consistent severity levels —
  `error!` on operation failure, `warn!` on degradation / defensive catches, `info!` on lifecycle
  (load / save / build / generate / adapter selection), `trace!` on per-stamp brush diagnostics.
  UI error strings render full `#[source]` chains via `{e:#}`. Boot logs include backend, adapter
  name, vendor, and device type so bug reports include the GPU context out-of-the-box.

> **STATUS UPDATE 2026-05-17:** NFR-Toolchain corrected — ADR-004 replaced
> nvdxt.exe with AMD Compressonator (ADR-014 vendors it separately under
> `tools/compressonator/`). NFR-Observability added after the Stage 1
> logging audit; convention is documented in ADR-018 / ADR-019 / ADR-020.
> NFR-Crash safety (autosave) is still aspirational — Phase 2+ work.

### 3.4 Architecture (conceptual)

```
[ UI Layer: egui ] ────────────────────────────────────┐
        │                                              │
[ Project Model ] ── undo/redo, autosave             [ 3D Preview ]
        │                                            (wgpu pipeline:
        ▼                                             heightmap mesh,
[ Map Data Core ]                                     DNTS shader approx,
  ├─ Heightmap (tiled COW, GPU-resident R16 texture)  feature billboards)
  ├─ Splat distribution (4-channel)
  ├─ Diffuse / specular / normal
  ├─ Metal/Type/Grass overlays
  ├─ Feature list (typed records)
  ├─ mapinfo.lua AST
  │
        ▼
[ Compile / Package Pipeline ]
  ├─ Image dump (PNG 16-bit, BMP, DDS-via-bc1)
  ├─ PyMapConv subprocess (sidecar)
  ├─ Lua serializer
  └─ 7z non-solid packager → mymap.sd7
        │
        ▼
[ Recoil Launcher ] → spring with --map flag
```

### 3.5 Data flow (terrain edit → playable)

1. User drags brush; UI emits a sequence of `BrushStamp { world_x, world_z, radius, strength }` values, one per frame the pointer is held.
2. **STATUS UPDATE (ADR-018 / ADR-021):** stamps are applied by **CPU kernels** in `barme_core::brushes` (the GPU compute path described in earlier drafts is formally deferred — bench shows ~10× headroom in the CPU implementation; see ADR-021). The CPU `Heightmap` is the source of truth; the affected pixel rect is sub-uploaded to the GPU r16uint storage texture via a single `queue.write_texture` call (ADR-017).
3. Symmetry replicates each stamp into N derived centers via `SymmetryAxis::replicate`; their dirty rects union into one upload (ADR-019).
4. Vertex shader samples the GPU heightmap texture per-vertex every frame; the per-frame uniform carries the camera matrix + `max_height` (changing height-scale costs zero buffer churn).
5. On Save: in-memory `Heightmap` flushed to a sibling PNG; project manifest written as `<name>.barmeproj` TOML. On Build: in-memory heightmap serialised to a temp PNG → PyMapConv subprocess → mapinfo emitter → 7-Zip non-solid packaging → optional install into BAR's user maps dir (ADR-015).

### 3.6 User stories (top 5)

- *As a new mapper*, I want a "Quicksilver-like 16×16 starter" template so I can iterate without reading the wiki.
- *As a competitive mapper*, I want guaranteed pixel-exact 4-way rotational symmetry so my map is tournament-acceptable.
- *As a mapper testing variants*, I want a "Reroll with seed" button on procedural terrain that preserves my symmetry and metal-spot intent.
- *As a returning mapper*, I want to drop an existing `.sd7` onto the editor and continue editing it.
- *As any mapper*, I want one button to compile and launch BAR with my map loaded.

### 3.7 Risk register

See §2.2.

---

## Phase 4 — Tech Stack Recommendation

### Decision: Rust + egui/eframe + wgpu, PyMapConv as sidecar.

| Stack | CrossPlat | Perf | GUI maturity | Single-binary | mapconv glue | Verdict |
|---|---|---|---|---|---|---|
| **Rust + egui/eframe + wgpu** | ★★★★★ | ★★★★★ | ★★★★ | ✅ static-linked | ✅ subprocess | **CHOSEN** |
| Tauri + WebGPU + Rust backend | ★★★★ | ★★★★ | ★★★★★ | ⚠ needs WebView2/WebKitGTK | ✅ | Runner-up; WebKitGTK rendering inconsistency on Linux is the killer for a heavy 3D viewport |
| C++ + Qt6 + OpenGL/Vulkan | ★★★★ | ★★★★★ | ★★★★★ | ⚠ Qt6 LGPL implies dynamic link or commercial license for true single-binary | ✅ | Industrial-strength but slow iteration, complex Linux packaging |
| Python + PyQt6 + ModernGL | ★★★★ | ★★ | ★★★★ | ❌ PyInstaller bundles are 100–300 MB, slow startup | ✅ trivially | Tempting because PyMapConv is already Python, but brush latency at scale is the show-stopper |
| Electron + Three.js | ★★★★ | ★★ | ★★★★★ | ❌ 150 MB+ | ✅ | Rejected: heavy terrain in WebGL inside Chromium violates "performant" |
| **Godot 4 as a tool** | ★★★★ | ★★★★ | ★★★ | ✅ exports single binary | ⚠ subprocess works but is awkward | Strong contender; built-in 3D viewport, GDScript productivity. **The credible alternative if Rust's learning curve is too steep.** |
| Unity as a tool | ★★★★ | ★★★★ | ★★★★ | ⚠ requires runtime | ✅ | Rejected: licensing, runtime distribution, "single executable" is a fiction in Unity. (Note: `tebeer/BARMapEdit` chose this path and stalled at 22 commits with zero users.) |
| Java + JavaFX | ★★★ | ★★★ | ★★★ | ⚠ requires JRE or jlink image (~80 MB) | ✅ | Rejected: SpringMapEdit tried it, died |
| Pure web app (WebGPU) | ★★★ | ★★★★ | ★★★★ | ❌ cannot package PyMapConv | ❌ | Rejected: the build step needs native binaries |

### Why Rust + egui + wgpu specifically

1. **Single static binary** on Windows (MSVC) and Linux (musl). No bundled runtime, no install step.
2. **wgpu** is the right abstraction for a heightmap editor's compute pipeline: compute shaders for brush kernels, R16 storage textures, indirect rendering for feature instancing, and a clean fallback from Vulkan/Metal/DX12 to GL.
3. **egui/eframe** is production-proven in this exact niche: per `rerun.io/docs/howto/visualization/extend-ui`, the **Rerun Viewer** ("built on top of egui and wgpu") visualizes 3D point clouds, tensors, and images in real time — and egui's CTO/author works on Rerun. egui's immediate-mode model matches the "describe UI from project state every frame" pattern that suits an editor with heavy undo/redo.
4. **PyMapConv subprocess** is trivial via `std::process::Command`. Bundle PyMapConv's PyInstaller exe under `tools/pymapconv.exe` (Windows) and `tools/pymapconv` (Linux), plus `nvdxt` under `tools/`. The user installs *nothing*. — **STATUS UPDATE 2026-05-17 (ADR-011/ADR-014):** Linux bundling is two trees, not one: PyMapConv at `tools/pymapconv/` and AMD Compressonator at `tools/compressonator/`. Both fetched by sha256-pinned scripts (`scripts/fetch-pymapconv.sh`, `scripts/fetch-compressonator.sh`). The `nvdxt` mention is obsolete (ADR-004 — Linux uses Compressonator).
5. **Containerization fallback:** Recoil's headless mode and PyMapConv both run cleanly under Docker, so a CI image is straightforward for headless build/test of generated maps. `beyond-all-reason/maps-metadata` already ships a Dockerfile-based build flow (`docker run -it --rm -v $(pwd):/build maps-metadata-build`) you can mirror.
6. **Performance budget verified by precedent:** Rerun handles real-time 3D scene visualization on egui + wgpu; wgpu compute on a 1025² R16 heightmap is sub-millisecond.

### Fallback: Godot 4 (C# or GDScript)

If the wgpu compute curve is too steep, Godot 4 exports a single executable for Windows/Linux, ships a robust 3D viewport, and has the **HTerrain plugin by Zylann** (`github.com/Zylann/godot_heightmap_plugin`, Godot 4.1+), whose documentation states it "supports level of details on the geometry using a quad tree … divided in chunks of 32×32 (or 16×16 depending on your settings)" — exactly the LOD/chunked heightmap rendering needed here. Call PyMapConv via `OS.execute`. The cost is a less-polished docking UI and a binary size of ~93 MB on Windows with default Godot export settings (reducible to ~35 MB with build flags, per `popcar.bearblog.dev` benchmarks) versus 15–25 MB for a Rust + egui binary.

### Existing-editor reuse decision

- **Fork PyMapConv:** No — use it as-is, vendored. It already works.
- **Fork SpringBoard:** No — it is Lua + in-engine; pulling its rendering out of Spring is a multi-month task.
- **Fork tebeer/BARMapEdit:** No — no license, no README, no community, Unity-based. Take it only as evidence that a Unity+ImGui approach is technically possible but lonely.
- **Fork hendkai/bar-map-generator:** No — it is a web procedural generator, not an editor.
- **Build the new tool fresh in Rust + egui + wgpu, calling PyMapConv as a sidecar.** This is the verdict.

---

## Recommendations (staged)

**Stage 0 — Validation (2 weeks).** Build a small Rust prototype that (a) loads a 16-bit PNG heightmap, (b) renders it as a meshed terrain in wgpu, (c) writes a fake project to disk, (d) shells out to PyMapConv and produces a valid `.sd7`, (e) launches BAR with that map. Goal: confirm the sidecar contract works end-to-end. **Go/no-go gate.** If PyMapConv on the target platform cannot be driven headless reliably, fall back to Godot 4 + HTerrain.

**Stage 1 — MVP (3–4 months).** Implement F1–F12. Ship a Windows `.exe` and a Linux AppImage. Have Beherith (or another active BAR mapper) review the output `.sd7` byte-for-byte against PyMapConv reference output for three test maps. Publish on `beyondallreason.info/guide/mapmaking-resources` as a beta tool.

**Stage 2 — v1 (additional 4–6 months).** Add F13–F17. Add a procedural template library matching popular BAR maps (Quicksilver, Glitters, Throne, Supreme Isthmus archetypes). Add a "Lint My Map" pass that catches all ten silent mapinfo.lua pitfalls in §2.1.

**Stage 3 — v2 (open-ended).** F18–F20, plus a "publish to BAR" wizard that opens a PR against `beyond-all-reason/maps-metadata` with the generated YAML row.

**Thresholds that change the plan:**
- If PyMapConv stops being maintained, or if Beherith refuses redistribution, **pivot** to an embedded Rust-native SMF/SMT writer using the `texpresso`/`bcdec` crates; add ~2 months.
- If Recoil ever changes the SMF binary format (it has not in 10+ years, but Recoil is forking actively), the embedded writer must follow.
- If brush latency on Linux + Intel iGPU exceeds 16 ms at 32×32 maps, drop to a CPU-tile-update path with a coarser preview LOD.

## Caveats

- **License clarity on PyMapConv is unresolved.** The repo carries no SPDX license file. Get explicit written permission from Beherith before redistribution; this is non-negotiable for a public release. — **STATUS UPDATE 2026-05-17 (ADR-003):** resolved; upstream now carries `CC0-1.0`. Redistribution is unblocked.
- **The "final look" in BAR will diverge from in-editor preview** because Recoil's actual ground shader (DNTS + PBR + atmospheric scatter + dynamic shadows) is far more complex than what a tool should reimplement. Document the gap up front; do not pretend WYSIWYG.
- **No standalone GUI map editor for BAR is currently maintained.** The closest active artifact — `tebeer/BARMapEdit` — has 0 stars, no README, no license, 22 commits, and is invisible to the BAR mapping community. This is an opportunity, not a threat: every existing guide opens with Beherith's warning, *"THERE IS NO INGAME MAP EDITOR FOR BAR."*
- **Recoil is an actively forking engine.** Pin against a Recoil release tag in CI and re-test on every release.
- **World Machine remains the de-facto procedural backbone for top-tier BAR maps.** A v1 editor will not displace it for elite mappers; aim instead at the long tail of mappers who today bounce off the toolchain entirely.

---

### Plan-completion table

| Plan item | Covered in |
|---|---|
| 1. BAR/Spring/Recoil map format details | §1.1–§1.3 |
| 2. Existing toolchain inventory | §1.4 |
| 3. BAR-specific requirements | §1.3, §1.5 |
| 4. PA reference UX | §1.6 |
| 5. Hidden pitfalls | §2.1, §2.2 |
| 6. Tech stack pros/cons | §4 table + Why Rust + Fallback |
| 7. Existing partial editors evaluation | §1.4 + Reuse decision in §4 |
| 8. SRS construction | §3 |
| 9. Targeted subagent (existing standalone editors) | informed §1.4 and §4 reuse decision |
| 10. Enrich + complete | done |

---

### External references (curated; checked into SRS so future sessions don't re-derive)

Repos worth keeping eyes on or borrowing patterns from:

| URL | What it is | Why we care |
|---|---|---|
| [Beherith/springrts_smf_compiler](https://github.com/Beherith/springrts_smf_compiler) | PyMapConv source (CC0) | Our sidecar (ADR-002 / ADR-011). v0.6.3 vendored. Open issues filed there if we discover bugs. |
| [GPUOpen-Tools/compressonator](https://github.com/GPUOpen-Tools/compressonator) | AMD's DXT/BC compressor (MIT) | Vendored under `tools/compressonator/` (ADR-014). PyMapConv shells out to `CompressonatorCLI` by name on Linux. |
| [beyond-all-reason/RecoilEngine](https://github.com/beyond-all-reason/RecoilEngine) | Recoil engine (BAR fork of Spring) | Authoritative source for: SMF binary layout, `CArchiveScanner` map-key rules (`rts/System/FileSystem/ArchiveScanner.cpp`), VFS Lua API. |
| [beyond-all-reason/BYAR-Chobby](https://github.com/beyond-all-reason/BYAR-Chobby) | BAR's lobby/menu UI in Lua | Source for the unofficial-map filter (`LuaMenu/widgets/gui_maplist_panel.lua`), `GetMinimapImage` (`LuaMenu/widgets/chobby/components/configuration.lua`), and the auto-minimap extraction handler (`LuaMenu/widgets/api_map_handler.lua`). |
| [beyond-all-reason/Beyond-All-Reason](https://github.com/beyond-all-reason/Beyond-All-Reason) | BAR's game mod (units, gadgets, widgets) | Source for the mapinfo-reading gadgets (`luarules/gadgets/`). When a gadget crashes on missing mapinfo fields, look here. First example we hit: `unit_sunfacing.lua`. |
| [gist: burnhamrobertp / bar-map-archive-format.md](https://gist.github.com/burnhamrobertp/97cae4d300e675ca261e661fc58266d1) | BAR map archive format reference | Quotes the *engine* minimum mapinfo (`name`, `smf.smtFileName0`, `teams[*].startPos`). Note: does NOT cover BAR-mod gadget requirements — see PITFALLS for the union. |
| [Jandodev/bar-editor](https://github.com/Jandodev/bar-editor) | Web/WebGL BAR map editor (MIT) | Independent project; Vue + Three.js + TypeScript. Useful as a reference for native-Rust SMF parsing (their TS SMF reader is small and clean) and Stage 2 brush algorithms (Flatten/Erode/Terrace). Different architecture from ours; not a competitor. |
| Beherith's *Advanced SpringRTS Mapping Guide* (Google Doc, linked from `beyondallreason.info/guide/mapmaking-resources`) | Hand-written map-authoring guide | The canonical BAR mapper onboarding doc. Pipeline: World Machine → PyMapConv → SpringBoard → 7z. |
| BAR maps directory (per-user) `~/.local/state/Beyond All Reason/maps/` (Linux) | User-installed `.sd7` files | This is the install target for Stage 0 goal #7's launcher. Real-map examples can be copied into `scratch/bar-maps/originals/` for reference. |