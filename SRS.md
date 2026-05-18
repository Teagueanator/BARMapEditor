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

### 1.4 Existing toolchain (active vs. legacy)

| Tool | Stack | Status | Role |
|---|---|---|---|
| **PyMapConv** (Beherith / `springrts_smf_compiler`) | Python 3 + PyQt + nvdxt.exe | **Active, canonical.** Forum consensus 2021–2025: "deprecate all other mapconvs and make pymapconv 'the' mapconv." | SMF + SMT compile/decompile, GUI + CLI, ships a Windows one-file `.exe`; Linux runs from source — **STATUS UPDATE 2026-05-17 (ADR-011):** v0.6.3 now ships a self-contained Linux ELF bundle (PyInstaller; ~90 MB extracted, bundles Compressonator-derived DXT encoders + ImageMagick at `tools/`). No Python 3, PyQt, Pillow, or external Compressonator/ImageMagick install required on Linux; the "runs from source" caveat is gone for the prebuilt path. Upstream `--help` is broken in v0.6.3 (argparse crash); flag surface is captured in `devlog/stage-0-validation/logs/2026-05-17T16-57-48__pymapconv-vendoring.md`. |
| **SpringBoard** (gajop / `Spring-SpringBoard`) | Lua, runs *inside* Spring/Recoil | 0.5.3 announced by gajop on 23 Sep 2017, last forum activity 6 Dec 2018; BYAR variant exists but is inactive | Most feature-complete editor: heightmap raise/set/smooth, DNTS/specular/diffuse painting, void tool, feature & unit placement, undo/redo. *In-engine, not standalone.* |
| **SpringMapConvNG** (tizbac) | C++ + DevIL | Legacy (last meaningful work 2023) | Cross-platform CLI compiler; historical Win32 free() crash |
| **SpringMapEdit** (Heiko Schmitt → aeonios) | Java + SWT + JOGL | Abandoned (~2009–2012) | Standalone 3D editor: brushes, hydraulic/thermal erosion, auto-texture, mirror/flip/shift; no metal/feature/sd7 |
| **World Machine** | Commercial Windows app | Active | Procedural terrain + texture generator; Beherith ships a `.tmd` template for BAR. CPU/RAM intensive (16 GB RAM for a 16×16 final render) |
| **hendkai/bar-map-generator** | Web JS UI + Python | Early (2024–2025), self-described unfinished | Procedural generator that shells out to PyMapConv; not an editor |
| **tebeer/BARMapEdit** | Unity (C#) + Dear ImGui + custom HLSL | Personal/dormant: 22 commits, 0 stars, no LICENSE, no README, not on the official BAR mapmaking-resources page | Earliest-stage standalone GUI attempt. **Not a viable fork base.** |
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

### 3.3 Non-functional requirements

- **NFR-Performance:** Brush stroke latency ≤ 8 ms on a 16×16 map at 60 fps preview on a mid-range 2020 GPU.
- **NFR-Memory:** Resident set ≤ 4 GB editing a 16×16 map; ≤ 8 GB at 32×32.
- **NFR-Portability:** Single static binary on Windows x86_64 and Linux x86_64; AppImage for Linux. No system-wide install required.
- **NFR-Toolchain:** Bundle PyMapConv + nvdxt under a `tools/` subdirectory of the executable.
- **NFR-Determinism:** Same project → byte-identical `.sd7` (compile timestamps stripped).
- **NFR-Crash safety:** Autosave every 60 s to disk-backed project file.
- **NFR-Audit:** Emitted mapinfo.lua is human-readable, diff-friendly, and matches BAR community style.

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

1. User drags brush; UI emits `BrushStroke(world_xz, radius, power, mode)`.
2. Stroke is dispatched to a wgpu compute shader that modifies the tiled R16 heightmap texture in place; affected tiles are marked dirty in the CPU mirror.
3. Symmetry post-pass replays the stroke into mirrored tiles.
4. Preview mesh tessellation reads the heightmap as a GPU texture every frame.
5. On Save: dirty CPU tiles flushed to project file. On Build: PNG export → PyMapConv subprocess → 7z non-solid packaging.

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
4. **PyMapConv subprocess** is trivial via `std::process::Command`. Bundle PyMapConv's PyInstaller exe under `tools/pymapconv.exe` (Windows) and `tools/pymapconv` (Linux), plus `nvdxt` under `tools/`. The user installs *nothing*.
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