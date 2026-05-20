# Roadmap

Mirrors SRS §3.2 (functional reqs) and the staged plan in SRS "Recommendations".
Treat this as the *engineering* breakdown; the SRS is the *product* spec.

## Stage 0 — Validation prototype (target: 2 weeks) — ✅ COMPLETE 2026-05-17

Go/no-go gate before committing to the full build. If anything here proves
unviable, the SRS prescribes a fallback to Godot 4 + HTerrain.

- [x] Repo scaffolded (Rust workspace, two crates, docs)
- [x] Rust toolchain installed (rustup, stable 1.95)
- [x] `cargo check` clean on workspace
- [x] `cargo run -p barme-app` opens a window
- [x] Load a 16-bit PNG heightmap from `assets/fixtures/`
- [x] Render it as a meshed terrain via wgpu (single draw call, no LOD yet)
- [x] Serialize a project to TOML on disk, reload it
- [x] Vendor PyMapConv under `tools/pymapconv/` (also Compressonator under
      `tools/compressonator/` — see ADR-014)
- [x] Shell out to PyMapConv with a fake-project export → produce a valid `.sd7`
- [x] Launch BAR with that `.sd7` selected and verify it loads in-engine
- [x] Decision recorded in `docs/DECISIONS.md`: **PROCEED with Rust stack**
      (ADR-016)

Stage 0 surprises that informed Stage 1 scope (full list in ADR-016):
three-gate mapinfo model (engine / Chobby / mod gadgets each have
independent requirements); Compressonator is a separate vendor from
PyMapConv; PyMapConv v0.6.3 exits 1 on success and needs `-q 1` to dodge
a multi-thread read-back bug; Chobby filters unofficial maps out of the
multiplayer browser (Skirmish-only).

## Stage 1 — MVP (3–4 months)

Implements SRS F1–F12. Ships a Windows `.exe` and a Linux AppImage.

- [x] **F1** New-project wizard — name + rectangular `smu_x × smu_z`
      + symmetry preset + biome preset (4 procgen-backed presets) +
      max height. Modal at app launch, re-openable via File → New
      project (ADR-024).
- [x] **F2** Real-time heightmap sculpting — raise / lower / smooth shipped via
      plugin-shaped `Brush` trait (ADR-018). Flatten / erode / ramp drop in
      under the same trait as Stage 1.5+ work.
- [x] **F3** Symmetry enforcement — mirror H / V / Quad / both diagonals +
      rotational `fold ∈ 2..=12` (ADR-019). Arbitrary-axis line picker
      remains Stage 2.
- [x] **F4** Texture painting via DNTS splat channels (4 RGBA, ≤4 splat textures)
      — **D1 shipped 2026-05-18** (ADR-025 / ADR-027): 16-slot CC0
      ambientCG starter palette + `scripts/fetch-textures.sh` (sha256
      pinned, idempotent, `--check` HEAD-probe). Per-slot layout under
      `tools/textures/<NN-slot>/{diffuse.png, normal.png, meta.toml}`
      is the contract D3's registry depends on.
      **D2 shipped 2026-05-18** (ADR-026): `crates/barme-pipeline::dnts
      ::bake_dnts` produces BC3 / DXT5 DDS from a slot's `normal.png`
      (+ optional diffuse) via the vendored Compressonator;
      content-addressed cache under `tools/textures-cache/<sha>.dds`;
      Y-flip is a runtime knob (default OFF — ambientCG `_NormalGL`
      is already OpenGL convention per FINDINGS §7.4); the
      `diffuse_in_alpha` high-pass path is plumbed but untested in
      BAR until ADR-034.
      **D3 shipped 2026-05-18** (no ADR — mirrors ADR-018):
      `crates/barme-core::splat` lands the fixed-1024² RGBA
      `SplatDistribution` + object-safe `SplatBrush` trait + registry
      with `paint` / `erase` / `smooth` brushes. `PaintChannel`
      enforces `R+G+B+A ≤ 255`; brushes return dirty rects for D4's
      sub-upload pattern.
      **D4 shipped 2026-05-18** (ADR-036): `crates/barme-app::render`
      extends the bind group to 7 entries (splat distribution
      texture, 4-layer rgba8unorm slot diffuse array, SplatU uniform
      block); `terrain.wgsl` fragment stage composites the four
      diffuse layers via the engine's splatCofac math
      (`SMFFragProg.glsl:174-198`) — diffuse-only this sprint, with
      heightmap-derived normals + Lambert + ambient lighting and a
      heightmap-driven biome-gradient fallback when no slot is
      bound. `SMF_INTENSITY_MULT = 210/255` pre-applied CPU-side per
      FINDINGS §7.1. Five engine-fidelity deferrals (DNTS-normal
      blending, ADR-034 high-pass alpha, shadows, specular,
      sky-reflect/water/fog) listed in ADR-036 with promotion
      triggers.
      **D5 shipped 2026-05-18** (no ADR — reuses ADR-035 widgets +
      ADR-036 GPU): `barme_core::SplatConfig` persists per-channel
      slot bindings + `tex_scales` / `tex_mults` /
      `diffuse_in_alpha` on `Project`; in-memory
      `SplatDistribution` field rides along for the session (D6
      ships PNG sidecar persistence). Inspector rewrite drops the
      Phase-7 scaffolding for a TEXTURE LAYERS picker (slot
      thumbnails from `tools/textures/<NN-slot>/diffuse.png`,
      inline slot-picker grid), BRUSH section driven by the D3
      registry, PER-LAYER TUNING (`tex_scale` 0.0015..=0.05,
      `tex_mult` 0..=4), and a GLOBAL `diffuse_in_alpha` pill.
      Canvas LMB stamps via the D3 brush, fanned through
      `App::symmetry`. Mini-map gains a translucent splat overlay.
      Validation chip warns "DNTS: no specular" per FINDINGS §7.2.
      Splat undo deferred (4 MB > 100 MB cap).
      **D6 shipped 2026-05-19** (Sprint 12 / closes F4): a new
      `barme-pipeline::splat_pipeline` module wires DNTS bake per
      active channel + the splat distribution PNG + a grey BC1
      specular fallback into the `.sd7`. `mapinfo.resources` emits
      the SUBTABLE form of `splatDetailNormalTex` (PITFALL §15 /
      FINDINGS §1.8) — `{ "a.dds", ..., alpha = false, }` — with
      `splatDistrTex` / `specularTex` pointing at the staged paths.
      A new `LuaValue::Mixed { values, keyed }` AST variant renders
      the bare-positional + keyed shape real BAR maps ship; the
      legacy `splatDetailNormalTex1..4` numbered keys stay in the
      schema for hand-authored-import survival but D6 doesn't emit
      them. `build_sd7` takes a new `SplatBakeInputs` parameter the
      app resolves from its slot registry. Painted-in-editor splats
      now load + render in BAR.
- [x] **F5** Metal-spot placement — Sprint 11 / C4 lands `Project.metal_spots:
      Vec<MetalSpot>` + the BAR-convention `extractor_radius` (80 elmos
      default; PITFALL §6 surface). Inspector renders a per-spot table with
      X / Z / metal `DragValue`s (metal range 0..=50 so the user can type any
      strategic value — 0.5 perimeter, 2.0 standard, 4.0 / 5.2 central — not
      capped to a slider). Canvas LMB places, LMB-drags moves, RMB deletes,
      with one `ProjectDiff::PlaceMetalSpot` per symmetry-replicated source so
      undo peels mirrors one at a time. Markers render as red filled circles
      with a cyan extractor-radius ring when the tool is active; ghost at
      50 % alpha when other tools are active (B1 cross-tool pattern).
      Pipeline emits `mapconfig/map_metal_layout.lua` `spots = { [N] = { x,
      z, metal } }` (integer-keyed for diff-friendliness) plus an all-zero
      `(32 * smu_x) × (32 * smu_z)` metalmap PNG passed to PyMapConv via
      `-m` — PITFALL §13 + FINDINGS §5: the BAR
      `map_metal_spot_placer.lua` gadget bails on any non-zero metalmap
      pixel, so this forces our Lua spots to be the source of truth.
- [x] **F6** Geo-vent placement — Sprint 11 / C5 lands `Project.geo_vents:
      Vec<GeoVent>` and the inspector / canvas plumbing mirroring F5's
      pattern. Pipeline emits each vent as a `{ name = "geovent", x, z,
      rot = "0" }` entry in `mapconfig/featureplacer/features.lua` —
      PITFALL §14 + FINDINGS §5–§6: BAR's `api_resource_spot_finder.
      GetSpotsGeo()` scans `Spring.GetAllFeatures()` for
      `FeatureDef.geoThermal = true`, so the stock `geovent` FeatureDef
      simultaneously renders the steam plume AND registers the spot with
      the resource-spot-finder upget. The Zero-K-style `geos = {}` array
      in `map_metal_layout.lua` is NOT a BAR convention and the metal-
      layout emitter explicitly suppresses it (regression-tested).
      `rot` is string-quoted per PITFALL §6 (FINDINGS §6 confirms the
      BAR-mapper convention).
- [x] **F7** Feature placement (trees, rocks, wreckage) into a Lua gadget —
      Sprint 12 / C6 lands `Project.features: Vec<FeatureInstance
      { name, x_elmo, z_elmo, rot_heading }>` + a new `Tool::Feature`
      (keyboard `F`; the geo-vent tool moved to `V`). Inspector
      surfaces a category combo + filtered picker driven by
      `assets/mapfeatures_catalog.json` (a hand-curated 30-entry
      baseline sourced from `github.com/beyond-all-reason/mapfeatures`,
      auto-gen is a polish task). Canvas LMB places; LMB-drag rotates
      (heading delta = dx × 182, ~1° per pixel); RMB deletes.
      Symmetry replicates sources — rotational fold N spins each
      mirror by `65536 / fold` so per-sector visuals stay symmetric.
      `featureplacer::object_entries` emits geo vents first (sorted
      by `(z, x)`) then user features (sorted by `(name, z, x)`) into
      the Springboard `set.lua`'s `objectlist`. Rotation is an
      UNQUOTED Lua integer (PITFALL §23 — the gadget's
      `Spring.CreateFeature(..., fDef.rot)` expects a number;
      PyMapConv's `-k` text-file format that uses quoted strings is
      a separate codepath). Unknown FeatureDef names don't gate the
      build (engine logs + skip; C8 will lint at Sprint 14).
- [x] **F8** Start-position editor — Phase 2 ADR-023 shipped the flat
      `Vec<StartPosition>` model; Phase 3 ADR-032 (B6) supersedes the
      data shape with `Project.ally_groups: Vec<AllyGroup>` (id + name
      + sRGB colour + sources + `box_polygon`). Inspector becomes a
      collapsing-header tree with configuration-preset dropdown
      (`1v1` / `8v8` / `3-way FFA` / `4-way FFA`). LMB-drag distributes
      N markers along the vector; hover↔pulse links tree to canvas;
      markers ghost cross-tool (B1 pattern). Mirrors live in the same
      ally group as the source; build path expands sources through the
      active symmetry before emission. Pre-Phase-3 `.barmeproj` files
      migrate via custom `Deserialize` (legacy `[[start_positions]]`
      → `ally_groups[0]`). C2 / ADR-029 wires the ally tree into
      `mapconfig/map_startboxes.lua`.
- [ ] **F9** `mapinfo.lua` editor (form + raw Lua tab)
- [ ] **F10** Minimap auto-generation
- [ ] **F11** One-click `.sd7` build via PyMapConv
- [ ] **F12** "Launch in BAR" button (invokes Recoil with `--map`)
- [x] **Editor maturity (Phase 2 closer)** — undo/redo over dirty-rect
      snapshots with stroke coalescing + barriers on procgen/load/new
      (ADR-022), Ctrl-Z / Ctrl-Shift-Z keybinds, Edit menu, 100 MB ring
      cap. The exploratory edit→evaluate loop is no longer one-way.
- [x] **Editor layout shell (Phase 3 / B1, ADR-030)** — five-zone
      panel structure: top action bar (menus + symmetry chip + Build),
      bottom status strip (camera readout + map dims), left 40 px tool
      strip (`Tool { Select / Sculpt / StartPositions / Procgen }` via
      Q/B/S/G accelerators), right 300 px resizable Inspector
      (persistent header + exhaustive `match self.tool` on tool params),
      CentralPanel last. Drag threshold bumped to 8 px. All F2 / F3 /
      F8 / F14 UI surfaces re-homed; nothing functional removed.
- [x] **Canvas affordances (Phase 3 / Sprint 3 = B2 + B3 + B4)** —
      symmetry canvas overlay (dashed axes / rotational spokes +
      mirror-brush ghost rings, ADR-031), primary brush ring at
      cursor (colour by brush), nav-gizmo top-right of viewport
      with 6 axis snaps + drag-orbit, first-launch hint Window
      persisted in a new per-user `EditorConfig` TOML keyed by
      editor version, `?` cheat-sheet modal auto-generated from
      `Tool::ALL` + camera bindings, top-bar primary Build button
      with variant `ComboBox` (Launch greyed pre-F12), 1 Hz status
      strip repaint. New module `crates/barme-app/src/config.rs`
      + `crates/barme-app/src/ui/{overlay,gizmo,cheat_sheet,intro}.rs`.
      `barme-app` test count 18 → 112.
- [x] **Data-model foundations (Phase 3 / Sprint 4 = B5 + C1)** —
      unified undo channel for non-heightmap edits
      (`enum HistoryEntry { Heightmap, Project(ProjectDiff) }`,
      no new ADR; extends ADR-033) so F8 placements + F1 wizard
      applies are undoable on the same Ctrl-Z stack as brush
      strokes; largest-first eviction keeps long strokes from
      evicting kilobyte-sized diffs. Typed `mapinfo.lua` schema
      (`MapInfo` + 9 sub-blocks + `MapInfo::bar_default()` +
      `From<&Project>`) at `crates/barme-core/src/mapinfo_schema.rs`,
      ADR-028, supersedes ADR-013's "minimum-viable" data-shape
      half. `Project.mapinfo_overrides: HashMap<String, toml::Value>`
      added (F9 / C7 will populate). Foundational only — B6 (F8
      allyteam redesign) and C2 (three-file emission) consume these
      next sprint.
- [x] **Three-file emission + F8 allyteam tree (Phase 3 / Sprint 5 =
      C2 + B6)** — ADR-029 swaps the string-concat mapinfo emitter
      for a Lua AST (`barme-pipeline::lua_ast`) walking the C1 schema;
      three sibling emitters land for `mapconfig/map_metal_layout.lua`
      (placeholder until C4 / C5), `mapconfig/map_startboxes.lua`
      (populated from `Project.ally_groups[*].box_polygon`),
      `mapconfig/featureplacer/features.lua` (placeholder until C6).
      All four files stage into the `.sd7` via the existing
      `barme-pipeline::sd7::package` machinery. Determinism pinned by
      per-emitter byte-identical-repeated-render tests. ADR-032 / B6
      replaces flat `Project.start_positions` with `ally_groups:
      Vec<AllyGroup>`; F8 Inspector becomes a tree with configuration
      presets (`1v1` / `8v8` / `3-way FFA` / `4-way FFA`); LMB-drag
      paints N markers along a vector; hover↔pulse links tree to
      canvas; cross-tool ghosting at 50 % alpha. Pre-Phase-3
      `.barmeproj` migration via custom `Deserialize`. ADR-013's
      emitter half + ADR-023's data shape are superseded.
- [x] **Defaults + procgen UX + demo state (Phase 3 / Sprint 6 =
      C3 + B7 + B8)** — three small finishing-touch items, no new
      ADRs. **C3:** `MapInfo::bar_default()` now seeds the digest's
      full BAR-convention block (lighting colours / shadow densities,
      atmosphere wind / fog / sky / cloud, `terrain_types` as a 4-
      entry vec — Default / Rock / Sand / Water). New
      `bar_default_with_water()` constructor for tidal / sub-zero
      maps. `sunDir` camelCase emission pinned by a regression test
      that also rejects the lowercase `sundir` form. **B7:** procgen
      Inspector reordered preset-first (preset dropdown → collapsed
      "Custom expression" → domain radio → 256² greyscale preview
      thumbnail → "Apply to heightmap"). Preview backs to a
      persistent `egui::TextureHandle` reused via `handle.set(...)`
      so the GPU page count stays flat across keystrokes. New
      `procgen::generate_thumbnail` helper + 50 ms debounce keyed
      on `hash(expr, domain)`. **B8:** `apply_wizard` seeds 2 demo
      start positions on N / S strips (with a valley-finder so
      parabolic-dome doesn't plant on the peak), reframes the
      camera at 35° pitch / 1.6 × diagonal, and pops a non-modal
      "Next steps" `egui::Window`. Wizard's default symmetry flipped
      from `None` → `Horizontal` so the N / S pair lines up.
      Dismiss persists per-project (new
      `Project.next_steps_dismissed` bool, NOT in `EditorConfig` —
      fresh projects re-show the hint). Test counts:
      barme-core 122 → 132 (+10), barme-app 114 → 117 (+3,
      including the moved B5 wizard test) + ui::next_steps (+2);
      barme-pipeline 49 → 52 (+3).
- [x] **Source audit emitter corrections (Sprint 10, devlog
      `stage-1-mapinfo-audit-fix`)** — closes the five emitter-side
      items the 2026-05-18 source audit (FINDINGS.md) flagged.
      Six surgical commits cite PITFALLS §11 / §12 / §18 / §19 /
      §20: dual-emit `sundir` + `sunDir` (gadget compat), rename
      `skyDir` → `skyAxisAngle` with serde migration of legacy
      `sky_dir` fixtures, fix `sunDir.w` default `1e9` → `1.0`
      (engine intensity scalar, not sunStartDistance), drop unused
      `gui.minimapRotation` (+ entire `GuiBlock`), add
      `voidAlphaMin` schema field with conditional emit on
      `voidGround`. 17 new regression tests; no behavioural splat
      churn (D6 / Sprint 12 wires `splatDetailNormalTex` subtable
      form separately).
- [x] **Renderer-parity foundation (Sprint 13 / ADR-037, devlog
      `stage-1-renderer-depth-rework`)** — opens the renderer-parity
      arc (Sprints 13 + 20–27, per
      `docs/research/renderer-bar-parity/ROADMAP.md`) by retiring the
      depth-less "2D-painter-on-flat-wgpu-pass" architecture. Seven
      commits: (1) offscreen `Rgba8UnormSrgb` colour + `Depth32Float`
      depth `OffscreenTarget` capped at 2048² per axis with
      egui-side texture registration; (2) terrain pipeline writes
      depth and rasterises into the offscreen RT via
      `Callback::prepare` (composited back as `ui.painter().image`);
      (3) `OrbitCamera::near_far` auto-tunes the depth window from
      orbit distance; (4) GPU marker pipeline + `MarkerBatch`
      (`crates/barme-app/src/markers.wgsl` + `ui/markers.rs`) with
      5 SDF shapes, premul blending, depth-test-only, back-to-front
      sort; start positions / metal spots / geo vents (incl. F7
      hit-tested but unrendered features) / brush rings all batch
      through it; (5) GPU line pipeline (`lines.wgsl`) for symmetry
      axes (world-space dashed) + geo-vent plumes + geo-vent mirror
      outlines (via `MarkerShape::OutlineTriangle`); (6)
      `world_to_screen` relaxed to only reject behind-camera so
      label projection agrees with the GPU rasterizer at the rect
      edge; (7) ADR-037 + this STATUS UPDATE. Test counts: barme-app
      157 → 211 (+54 over the sprint — marker batch + camera near_far
      + offscreen size resolution + world_to_screen relaxation +
      symmetry-segment collection). Markers now occlude against
      terrain; translucent markers blend in correct camera-relative
      order under orbit. Remaining parity work (DNTS lighting,
      atmosphere, water polish, shadows, S3O features, grass,
      emission, validation) is **Sprints 20–27**.
- [x] **STATUS UPDATE 2026-05-19 (Sprint 14 / C9 — ADR-042).** Water
      + Lava authoring shipped as a map-property tool. Closes the
      silent emission gap (`From<&Project> for MapInfo` always left
      `info.water = None`; `bar_default_with_water()` was dead code).
      Six commits on `main`: (1) `WaterMode` enum + per-preset
      `WaterBlock` literals anchored to real BAR maps (Coastlines,
      Gecko Isle, Acidic Quarry; synth Lava/Magma at the
      ground-block damage threshold) + `Project.{water_overrides,
      void_water, tidal_strength, schema_v}` with one-shot migration
      `min_height < 0 → Ocean`; (2) `water.wgsl` MVP — flat
      alpha-blended quad at Y=0, depth-test on / depth-write off,
      drawn between terrain and lines per Sprint 13's translucent
      contract; (3) `Tool::Water` (keyboard `W`, `Icon::Water` wave
      glyph) + `inspector_water` form (Preset chips / Behaviour /
      Appearance / Flood / Advanced) + `apply_brush_id_at` refactor
      so the Water tool reuses Sculpt's Brush::Lower + symmetry +
      undo machinery; (4) Lava/Magma atmosphere offer
      (`Project.lava_atmosphere: bool` + hardcoded fog/sun/cloud
      patch); (5) validation-chip warnings for DNTS+water LOS bug
      (PITFALL §8), terrain-vs-mode mismatch (both directions), and
      cross-tool ghosting at 0.5× alpha; (6) ADR-042 + this STATUS
      UPDATE. `App.min_height` now plumbs end-to-end (Stage-1 bug
      where `snapshot_project` hard-coded 0.0). Test counts:
      barme-core 166 → 196 (+30 water_presets module + project
      migration + emission merges + new ProjectDiff variants);
      barme-app 221 → 231 (+10 water_draw / cross-tool / validation /
      WaterU layout pins); barme-pipeline 110 → 114 (+4
      `surfaceAlpha` / `waveFoamIntensity` Lua emission). Polished
      water (foam / fresnel / caustics / lava
      emission / perlin wave motion) deferred to the renderer-parity
      arc as agreed in the C9 prompt's "Out of scope" section.
- [x] **STATUS UPDATE 2026-05-19 (post-C9 smoke).** First Tool::Water
      run surfaced three follow-ups; all fixed before this line
      lands. (1) The terrain shader ignored `Project.min_height` —
      water at `Y = 0` sat invisibly at the heightmap floor. Fixed
      by extending the terrain `Uniforms` with
      `params2: vec4<f32>` (`.x = min_height`) and rewriting
      `sample_y` to compute `y = min_h + t * (max_h - min_h)`. The
      fragment biome-ramp rescales so submerged terrain gets a
      distinct gradient band. (2) Inspector Flood section rewritten
      with an explainer card and a directly-editable
      **Sea-floor depth** DragValue replacing the previous
      Auto-set-only affordance. (3) Adjacent camera-UX gap:
      arrow-key pan (delta-time-scaled, Shift = 3× faster) +
      Compass-icon recenter button in the top bar + zoom-aware
      rulers (`viewport_chrome::paint_rulers` now camera-projects
      ticks back to world XZ via `screen_to_world_y0`, labels in
      `1-2-5 × 10^k` step sizes that adapt to visible range). The
      first arrow-key build had left/right inverted; new PITFALL §27
      documents the `glam::Mat4::look_at_lh` sign-flip that caused
      it, and PITFALL §28 captures the
      `Ground.h::GetWaterPlaneLevel` `consteval` constraint + the
      `min_height` shader-plumbing requirement. ADR-042 carries the
      same details in its post-C9 STATUS UPDATE block.
- [x] **STATUS UPDATE 2026-05-19 (Sprint 15 / D8 — ADR-038).**
      Layered texture stack data model + CPU bake shipped. New
      `barme-core::layers` module — `LayerStack`,
      `TextureLayer`, `LayerSource { Slot | Imported }`,
      `LayerTransform`, `LayerColor`, `BlendMode::Normal`,
      `LayerMask` (flat `Vec<u8>` + base64 TOML serde;
      tiled-COW lands in D9 / Sprint 16). The CPU compositor
      `LayerStack::bake_diffuse` is per-row rayon parallel with
      a second per-layer rayon `par_iter` for PNG decode,
      wallpaper-tiled bilinear sampling, mirror-then-rotate
      transform, alpha-over composite back-to-front. 4-SMU /
      2-layer smoke clocks ~72 ms — well inside the
      1.5 s / 16-SMU / 8-layer budget in ADR-038. `Project.layers`
      is now the source of truth for the exported diffuse;
      `synth_biome_bmp` survives as the empty-stack fallback
      (the `barme-pipeline` smoke example + integration tests
      build bare `Project`s and still hit it). Four new
      `ProjectDiff` variants (`AddLayer` / `RemoveLayer` /
      `ReorderLayer` / `SetLayerProperty`) plus a
      `LayerPropertyValue` enum; the 100 MB undo cap eviction
      stays honest by folding mask + string capacities into
      `ProjectDiff::bytes()`. `barme-app::launcher::
      build_and_install` grows a `SlotResolver` parameter and a
      three-way texture branch. New
      `crates/barme-app/examples/bake_layered_smoke.rs`
      exercises a two-layer composite end-to-end. Test
      counts: barme-core 196 → 221 (+25); barme-app 232 → 234
      (+2 launcher pins); barme-pipeline 114 unchanged. **No
      paint UI** — Sprint 16 (D9, ADR-039 / ADR-040) lands the
      top-down 2D paint viewport + GPU composite preview +
      tiled-COW masks; Sprint 17 (D10, ADR-041) lands the
      Photoshop-style Layers panel + custom texture import +
      DNTS hybrid emission (retires `inspector_splat` and
      `Tool::SplatPaint`).
- [ ] Beherith (or active mapper) reviews `.sd7` byte-for-byte against PyMapConv
      reference output on three test maps
- [ ] Listed on `beyondallreason.info/guide/mapmaking-resources` as beta

## Stage 2 — v1 (additional 4–6 months)

SRS F13–F17 plus quality-of-life.

- [ ] **F13** Decompile / import existing `.sd7`
- [ ] **F14** Procedural terrain — **math-function subset shipped early in
      Stage 1** via `barme_core::procgen` + `evalexpr` (ADR-020). Remaining:
      FBM, hydraulic erosion, river carve (each needs its own ADR).
- [ ] **F15** Type-map editor + per-terraintype gameplay params
- [ ] **F16** Skybox picker / atmospheric preset library
- [ ] **F17** Pathability overlay
- [ ] "Lint My Map" pass — catches all ten silent `mapinfo.lua` pitfalls in
      `docs/PITFALLS.md`
- [ ] Procedural template library (Quicksilver, Glitters, Throne, Supreme
      Isthmus archetypes)

## Stage 3 — v2 (open-ended)

- [ ] **F18** DEM (GeoTIFF) import
- [ ] **F19** Procedural feature scatter with rule sets
- [ ] **F20** "Publish to BAR" — opens a PR against `maps-metadata` with
      generated YAML row

## Pivot thresholds (from SRS)

- PyMapConv stops being maintained, or licensing reverses → embed Rust-native
  SMF/SMT writer via `texpresso` / `bcdec`. +2 months.
- Recoil changes SMF format → embedded writer must follow.
- Brush latency on Intel iGPU > 16 ms at 32×32 → drop to CPU tile-update with
  coarser preview LOD.
