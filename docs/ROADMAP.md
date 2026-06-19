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
- [x] **F9** `mapinfo.lua` editor (form + raw Lua tab) — Sprint 18 /
      C7 ships the typed-form editor opened from the top-bar
      `Icon::MapInfo` button. Non-modal `egui::Window` with 12 tabs
      (General / Map / SMF / Lighting / Atmosphere / Water /
      Resources / Splats / Terrain types / Custom / Raw Lua / Minimap).
      Edits commit on widget release through `ProjectDiff::EditMapInfo
      { from, to }`. New `MapInfoPatch` enum (49 variants) in
      `barme-core::mapinfo_schema`; `App::apply_mapinfo_patch` routes
      to first-class shadows or to `Project.mapinfo_overrides` as a
      dotted-Lua-path bag for fields without a shadow yet. Sprint 27
      will promote the most-edited atmosphere / lighting subset to
      typed shadows. Tab-strip lint-dot rendering stubbed at zero so
      Sprint 21 populates without UI change. Water tab is read-only
      in Sprint 18 — the dedicated `Tool::Water` Inspector remains
      the canonical entry; Sprint 26 polishes the form-side editing.
- [x] **F10** Minimap auto-generation — Sprint 18 / D7 ships
      `barme_pipeline::minimap::render_minimap` as a CPU bake
      (downsampled `LayerStack::bake_diffuse` + heightmap Lambert
      hill shade keyed on `mapinfo.lighting.sun_dir`). New
      `Project.minimap_override: Option<PathBuf>` with `SCHEMA_V`
      bump 1 → 2; when set the bake is bypassed and the user's PNG
      is copied verbatim after a strict 1024² dim check.
      `build_sd7` gains an `Option<MinimapInputs>` arg threaded
      through to PyMapConv's `-p` flag. Headless wgpu was scoped
      out per the kickoff devlog (the CPU input matches what the
      `.sd7` ships; a GPU render would only invite drift).
- [~] **F11** One-click `.sd7` build via PyMapConv — Sprint 20 / U3
      ships the async + visible-feedback half. Build runs on a worker
      thread; `BuildState` machine drives a centred progress overlay
      (current stage, sub-progress, MM:SS elapsed, Cancel button) and
      a live build log panel that streams PyMapConv + Compressonator
      lines through the new `invoke_with_streaming` primitive. Status
      strip mirrors Running / Done / Failed / Cancelled with
      click-to-show-log. `File > Recent projects (N)` submenu + an
      empty-state CTA recent-projects block surface the last 10
      opened-or-saved projects (cap 10, dedupe, missing files drop
      silently). Save-before-build modal gates dirty builds; disk-
      space check warns at <2 GB free without gating. The
      synchronous `launcher::build_and_install` path is retired —
      all build invocations go through the worker. Underlying
      compile contract (PyMapConv flags, artifact-presence success
      heuristic, non-solid `.sd7`) is unchanged from Sprint 12;
      Sprint 20 is pure UX / reliability.
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
- [x] **STATUS UPDATE 2026-05-19 (Sprint 16 / D9 — ADR-039 +
      ADR-040).** Layered painter UI + GPU live preview shipped.
      The user paints into per-layer masks via a new top-down 2D
      paint viewport (`Tool::PaintLayer`, keyboard `L`); strokes
      show up live in both the 2D viewport AND the 3D viewport
      (composite RT bound to the terrain shader's diffuse base).
      Tiled COW mask storage (`barme_core::layers::mask::TileGrid`)
      replaces Sprint 15's flat `Vec<u8>` — 256² tiles in either
      `Tile::Uniform(byte)` (~16 B resident) or `Tile::Pixels`
      (64 KB, allocated lazily). Fresh layers cost ~16 KB
      regardless of map size; brush strokes scale with paint
      coverage. Four mask brushes (`mask-reveal` / `mask-hide` /
      `mask-smooth` / `mask-fill`) under a new `MaskBrush` trait.
      GPU composite pipeline (`composite.wgsl`) alpha-overs up
      to 16 layers into an `Rgba8Unorm` RT clamped at 4096²
      (PITFALL §5 — bilinear upsample at terrain bind time for
      >8 SMU maps; CPU bake stays authoritative for `.sd7`). Per-
      layer dirty-tile sub-uploads (`dirty_tiles_since(version)`
      → `write_composite_layer_mask_tiles`) honour the 8 ms NFR
      — a 200×200 stamp uploads ~64 KB, not the 256 MB a full
      mask push would cost. Paint viewport (`ui::paint_view`)
      renders the composite RT into the central rect at 1:1
      aspect with letterboxed bands (PITFALL §8); pan = middle-
      drag, zoom = scroll wheel, double-click reset. Brush ring
      overlay + status strip + mask-only preview toggle. Fast-
      drag stamp interpolation (PITFALL §3) prevents gaps. Per
      user direction, Sprint 17's Layers panel was brought
      forward: add / rename / delete / reorder / opacity /
      visibility / texture import (picked-path; project-local
      sidecar still Sprint 17). Demo seed adds a second slot-1
      accent layer on fresh projects so painting reveal/hide
      immediately produces visible results. Tests: barme-core
      221 → 253 (+32 mask + brush invariants); barme-app 234 →
      247 (+13 composite uniform layout + RT clamp + paint-view
      auto-fit). 5 commits on `main`: tiled COW + brushes; GPU
      composite + offscreen RT; paint viewport + minimal strip;
      full Layers panel + demo seed; this rollup. Sprint 17 (D10
      / ADR-041) remains scoped to DNTS hybrid emission +
      `inspector_splat` retirement + project-local texture
      sidecar + drag-to-reorder + per-layer thumbnail + lock
      toggle + blend-mode selector + per-layer transform UI +
      mask-only preview's grayscale render + mask symmetry +
      per-stroke mask undo.
- [x] **STATUS UPDATE 2026-05-20 (Sprint 17 / D10 — ADR-041).**
      Layered painter trio complete. F4 closes end-to-end. The
      user's 2026-05-19 "the textures of the end map are quite
      incredibly ugly" report is resolved: the `.sd7` ships a
      composited diffuse from an unlimited stylistic layer stack
      at full texture resolution, and the bottom ≤4 DNTS-bound
      layers drive runtime per-fragment normal mapping in BAR.
      Full Photoshop-style Layers panel in `ui::layers_panel`
      (drag-to-reorder, lock chip, per-layer thumbnail, DNTS
      channel chip with R/G/B/A/∅ cycle + right-click picker
      + conflict-transfer, opacity slider, expanded properties
      for Source / Transform / Color / Blend / DNTS bindings,
      footer chips). User-driven addition mid-sprint: a stock-
      slot picker dropdown opens from the Add-layer primary
      action + the active-layer "Change slot…" affordance —
      `widgets::slot_picker_grid` extracted from the deleted
      `inspector_splat`. Stock textures are one click away;
      uploads are the secondary path. Custom imports copy into
      `<project>/textures/<uuid>.png` with a `meta.toml` sidecar.
      Load-time migration carries pre-Sprint-17 absolute paths
      forward. Mask brushes fan through `App::symmetry`. Mask-
      only grayscale preview chip renders (red where mask = 0).
      Per-stroke mask undo via tile-granular snapshots adapts
      ADR-033 onto Sprint-16's tiled-COW masks (`MaskEntry +
      OpenMaskStroke`, dedup-on-touch to avoid 15 MB/sec clone
      churn on continuous drags). DNTS hybrid emission
      (`stage_splat_assets_from_layers + materialize_splat_
      distribution_from_layers + populate_resources_from_layers`)
      box-filter-downsamples each DNTS-bound layer's mask to
      1024² + bakes a per-slot DDS via `bake_dnts`. Imported-
      source DNTS layers emit `LintWarning::ImportedLayerDnts`
      and skip the DDS bake. Runtime DNTS shader uniforms now
      derive from `LayerStack::dnts_layers()` (per-layer
      `dnts_tex_scale` / `dnts_tex_mult` / channel binding).
      Retired: `Tool::SplatPaint` variant + every match arm + ~470
      LoC of `inspector_splat` + ~90 LoC of `apply_splat_brush_at`
      + `App::splat_brush_state` + `SplatBrushState` struct +
      `App::splat_picker_open_for` + `App::splat_config` mirror +
      `App::splat_distribution` mirror + `App::splat_brushes` +
      `App::reupload_bound_slot_diffuses` +
      `App::reupload_splat_distribution`. `Project.splat_config`
      marked `#[serde(skip_serializing)]` — new saves drop the
      legacy block; old loads still hydrate via the wire-side
      default so `after_load_migrate` can seed the layer stack.
      `Tool::ALL` shrinks 10 → 9; keyboard `T` is freed.
      One-time migration toast on pre-Sprint-14 project open
      (dismissable, persists per-project via
      `Project.migration_toast_dismissed`). ADR-041 added;
      ADR-027 amended with the `<project>/textures/<uuid>.png`
      sidecar layout. Tests: barme-core 253 → 264 (+11 net: mask
      snapshot/restore, mask undo round-trip, imported-root
      resolution, splat_config-skip-serialize, layer DNTS
      uniforms + ProjectDiff bytes accounting); barme-pipeline
      114 → 117 (+3: box-filter downsample smoothness, RGBA
      invariant, imported-layer lint); barme-app 247 → 240 (−7
      legacy splat-painter tests deleted + the layers panel
      smoke-tests live end-to-end via the smoke run). Workspace
      total 614 → 621. 7 commits + 1 hotfix on `main`. Known
      Sprint 18 polish followups: GPU compositor extension to
      take per-layer color + transform uniforms (color slider
      edits don't reflect in the live RT preview today, only in
      the CPU bake at `.sd7` time), garbage collection of
      orphaned imported textures on undo, and a 16-SMU
      memory-pressure investigation (the user hit an OOM on
      Tool::PaintLayer entry; this rollup's dedup-snapshot fix
      mitigates the worst per-stamp allocation churn but
      doesn't root-cause the entry transient).
- [x] **STATUS UPDATE 2026-05-20 (Sprint 19 / U1).** UI
      discoverability + feedback pass. NEW `ui::help_text`
      module — 107 `HelpId` variants × matching strings, used
      by every Inspector tool's DragValue / ComboBox / Button /
      Slider / Chip via `.on_hover_text(help(HelpId::Foo))`.
      Hover-text coverage grep: 56 → 202 (target was >200).
      NEW `ui::lint_panel` stub (egui::Window) wired to both the
      top-bar validation chip and the status-strip issue-count
      label (replaces the hard-coded `"0 issues"`). NEW top-bar
      Help icon opens the cheat sheet. Save tooltip cites real
      Ctrl+S binding — chord wired in `handle_keyboard`
      alongside Ctrl+Shift+S = Save as. `viewport_chrome` drops
      the bogus `(G)` / `(L)` / `(W)` chord hints (those letters
      bind to tool accelerators). `brush_ring_color` extended
      from 3 sculpt brushes to 7 — mask-reveal / hide / smooth /
      fill pull from `Tokens::DARK` so they match the
      inspector_paint_layer brush card palette. Minimap symmetry
      guide rewrite: replaces the unconditional vertical
      bisector with `paint_minimap_symmetry` + the testable
      `minimap_symmetry_segments` helper that reads
      `Project.symmetry`. NEW `App::inspector_sticky_chips`
      prepends a Symmetry + Map size chip row to each tool's
      body. Tests: barme-app 253 → 263 (10 new — 4 lint_panel,
      8 minimap symmetry geometry, 2 mask-brush colours; 5
      help_text catalogue exhaustiveness / chord-binding tests
      from the first commit). 5 commits on `main`. Out of scope
      for Sprint 19: per-rule lint registry (Sprint 21 / C8),
      onboarding tour (Sprint 22), async build pipeline (Sprint
      20), inspector layout refactor (Sprint 27).
- [x] **STATUS UPDATE 2026-05-20 (Sprint 22 / U2).** Onboarding
      loop closure — the third and final UI/UX polish sprint
      (19 / 20 / 22). Five new surfaces wired into the Sprint
      19 / 20 / 21 framework. NEW `ui::help_center` module —
      33 inline-bundled markdown articles (4 meta + 9 tool + 3
      reference + 17 pitfall) baked via `include_str!`,
      rendered through a minimal in-module markdown subset
      renderer pinned to `Tokens::DARK` (egui_commonmark
      deferred — not in offline cache; Stage 2 polish can swap).
      NEW `ui::tour` module — 7-step guided walkthrough
      (project header → tools → inspector → canvas → minimap →
      status → help icon) with darkened backdrop, target
      cutout, 8s inactivity advance, and per-step `[Next]` /
      `[Skip tour]` callout. Auto-triggers on first new project;
      re-runnable from `Help > Start guided tour`. NEW
      `ui::tool_intro` — non-modal per-tool intro overlay on
      first entry; "Don't show again" checkbox persists to
      `EditorConfig.tool_intros_seen` only on explicit click
      (Esc-dismiss is temporary per critical pitfall #2). NEW
      `ui::command_palette` — Ctrl+K opens a centred Window with
      ≥40 commands (49 baseline registered); fuzzy substring
      filter; arrow / Enter / Esc; select-and-Enter is the only
      execute gesture (critical pitfall #3). NEW Ctrl+Shift+H
      "What's this?" hover-popover mode + `help_text::show_popover`
      helper + status-strip indicator chip (not persisted across
      restarts per critical pitfall #8). Entry-point wiring:
      top-bar Help icon → help center, lint-rule rows gain
      `[Help…]` → PITFALL article, build-log Failed header
      gains `[What does this mean?]` → BuildPipeline article,
      wizard next_steps Window gains `[Start the tour]`, Layers
      panel empty state gains `[How layers work]` →
      LayeredPainter article, Help menu (Open help center /
      Start guided tour / Reset tool intros / Cheat sheet /
      Command palette). `EditorConfig` extended with
      `tour_completed_for: Option<String>` +
      `tool_intros_seen: BTreeSet<String>` (forward-compat via
      `#[serde(default)]`; pre-Sprint-22 configs load cleanly).
      Tests: 50 new — 11 help_center (article count, paragraph
      floor, category split, pitfall round-trip), 11 tour, 9
      tool_intro, 12 command_palette, 5 apply_command, 2
      EditorConfig U2 fields. Workspace total: 281 → 331
      barme-app unit tests. 7 commits on `main`. Three UI
      polish sprints (19 / 20 / 22) now complete; Sprint 21's
      lint registry (C8) feeds the help-center wiring. Out of
      scope for Sprint 22: 16-SMU OOM root-cause + orphan-
      texture GC + legacy `SplatConfig` retire (now landed in
      Sprint 23 — see STATUS UPDATE below);
      animated tour callouts (text-only ships);
      runtime article loading (recompile-only for now);
      i18n / localisation; interactive sandbox tutorials;
      toast queue / proper modals (Sprint 31).
- [x] **STATUS UPDATE 2026-05-21 (Sprint 23 / T1, ADR-041 amendment).**
      Closes the three Sprint-17 followups carried since Sprint 17 /
      D10. **5 commits on `main`** so a bisect can attribute any
      regression to its triggering change:
      (1) investigation — RSS harness (`barme_core::rss`,
      `procfs`-backed, Linux-gated) + CPU layer-stack budget
      regression tests + characterisation of the pre-fix cold-sync
      contract;
      (2) H1+H4 mitigation — delete the 65 536-call row-by-row
      zero-fill loop in `render::ensure_composite_rt` (wgpu
      zero-initialises textures by default; the loop was wasted
      staging arena work);
      (3) H2 mitigation — `TileGrid::filled` seeds
      `current_version` / `tile_versions` at 0 when `fill == 0`
      (matches the GPU's zero-init default; `dirty_tiles_since(0)`
      returns empty on uniform-zero masks);
      (4) orphan-texture GC — new
      `barme_core::layers::garbage_collect_textures(project, root)`
      with `GcReport`; wired through `File > Garbage collect orphan
      textures` menu + auto-run after every successful save (silent
      when empty; `last_error` toast otherwise);
      (5) legacy `SplatConfig` retirement —
      `crates/barme-core/src/splat.rs::SplatConfig`,
      `Project.splat_config`, `LayerStack::migrate_from_splat_config`
      all deleted; legacy `.barmeproj` loads now flow through a new
      `barme_core::layers::legacy_splat_config_to_layers(value:
      toml::Value, size)` function that parses the on-disk
      `[splat_config]` table directly without a typed struct.
      `Project::after_load_migrate` takes the raw TOML text +
      returns `bool` so `App::open_from` can fire a one-time
      terminal banner. `SCHEMA_V` not bumped (serde's
      default-when-missing handles the field's absence).
      Tests: barme-core 274 → 277 (net delta covers the 3 deleted
      `SplatConfig` tests + the 4 new `legacy_splat_config_to_layers`
      and migration tests + the 6 GC tests + the 3 RSS / 4 cold-sync
      contract pins); barme-app 331 (unchanged top-line; one
      timing-sensitive `procgen` thumbnail test is flaky under load
      but pre-existing). Sprint 24 = multithreading (rayon procgen
      + parallel DNTS bake).
- [x] **STATUS UPDATE 2026-05-21 (Sprint 24 / T2).** First
      multithreading sprint — both PROPOSAL gates that crossed their
      promotion thresholds (procgen apply + DNTS bake) are now
      parallel. **4 commits on `main`**:
      (1) `core: parallelise procgen apply via rayon` —
      `procgen::generate` lifts to `par_chunks_mut` over rows; each
      worker constructs its own `PixelContext`; the `evalexpr::Node`
      is `Send + Sync + Clone` via the crate's `IsSendAndSync`
      compile-time assertion (defence-in-depth: `procgen.rs` carries
      a `const _: fn() = || { assert_send_sync::<…>() }` so an
      upstream evalexpr API change that drops the bound fails the
      build instead of silently regressing). NaN/Inf warn-once
      gate collapses to `AtomicBool`. Dev-box (12 cores): 16 SMU
      parabolic 440 ms → 40 ms (~11×), cone-peak 440 ms → 71 ms
      (~6×). `generate_thumbnail` stays serial.
      (2) `core: add bench-procgen example` —
      `crates/barme-core/examples/bench_procgen.rs` reports 5-run
      median wall-times at 4 / 8 / 12 / 16 SMU × parabolic / cone.
      Manual run; not in CI.
      (3) `pipeline: parallel DNTS bake + atomic cache write` —
      `stage_splat_assets_from_layers` bakes up to 4 channels
      concurrently inside a SCOPED rayon pool capped at
      `min(num_cpus, 4)` (no pollution of the global pool that
      procgen uses; no thrash on a 4-core dev box with 16 layers).
      New `on_progress: impl Fn(usize, usize) + Sync` parameter
      drives `BuildEvent::Progress(done/total)` into Sprint 20's
      build overlay; per-bake completion (not start), unordered.
      `dnts::bake_dnts_in_env` now writes the Compressonator output
      to a `tempfile`-managed sibling of `cache_path` (suffix
      `.dds` — `.tmp` triggers "Destination file type TMP is not
      supported") and atomic-renames via `TempPath::persist`
      (`fs::rename` POSIX, `MoveFileExW(REPLACE_EXISTING)` Windows).
      Two parallel bakes sharing a `cache_key` are now safe —
      last-writer-wins, but `cache_path` is never torn. Dev-box:
      single = 80 ms, parallel(4) = 97 ms, ratio = 1.21× (target
      < 1.5×). New PITFALL #29 documents the mid-sprint regression
      where a synthetic-layout test helper symlink-wrote `b""` into
      `tools/compressonator/compressonatorcli-bin`, corrupting the
      vendored ELF; both `locate_compressonator` helpers now reject
      zero-byte binaries so the failure surfaces as a clean skip
      rather than an `ENOEXEC` panic.
      (4) `brushes: TODO markers for deferred rayon lift` — brush
      kernels stay single-threaded (0.79 ms baseline well under
      8 ms NFR-Performance budget) but the inner loops in
      `raise.rs::apply_radial_delta` + `smooth.rs::apply` carry the
      one-line `par_chunks_mut`-per-row pattern in a TODO comment
      so the future lift (when 32-SMU support arrives or a user
      reports stroke lag) is mechanical. `lower.rs` is a one-line
      pointer comment since it delegates to `apply_radial_delta`.
      Tests: barme-core 277 → 279 (+2: par_serial determinism +
      parallel perf budget); barme-pipeline 212 → 213 (+1:
      parallel-bake regression). Sprint 25 = terrain shader parity
      (port SMFFragProg.glsl).
- [x] **STATUS UPDATE 2026-05-21 (Sprint 25 / R1, ADR-043 — opens the
      renderer-parity arc).** The Sprint-9 / D4 diffuse-only splat
      composite (ADR-036) is replaced by a line-by-line transcription
      of `SMFFragProg.glsl`'s `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING`
      branch. **4 commits on the sprint branch** (then a 5th rollup):
      (1) `render: extend terrain bind group + uniforms for SMFFragProg
      port` — `SplatUniforms` grows `ground_specular` + `camera_pos`;
      `flags.w` packs the texture-presence bitfield (bit 0 base
      normal, bit 1 specular, bit 2 DNTS normals); bind group adds
      base-normal + specular + slot-normal-array bindings with 1×1
      fallbacks so the layout never changes per frame.
      `RenderResources::heightmap_terrain_view` mirrors the existing
      `composite_terrain_view` fallback so `rebind()` is safe to call
      before a heightmap is uploaded. Five new tests pin
      `SplatUniforms = 128 B`, `SMF_INTENSITY_MULT = 210/255`, the
      engine-matching default specular `(0.1, 0.1, 0.1, 100.0)`, the
      texture format constants, and `camera_pos` default = origin.
      (2) `render: port SMFFragProg.glsl into terrain.wgsl` — base
      normal R+A decode (§7.5), per-fragment TBN from
      `cross(normal, vec3(-1, 0, 0))` (§7.4), full-RGBA `* 2 - 1`
      signed-decoded DNTS slot blend with per-channel `tex_scales` UV
      streams (§7.3), `splat_detail_strength.y = clamp(.a, -1, 1) ×
      diffuse_in_alpha`, normal blend in world space via the per-
      fragment STN matrix, Lambert + Blinn-Phong using
      `camera_pos.xyz - world_pos` for the half-vector, specular
      exponent `α × 16.0` with global fallback (§7.6). Each WGSL
      section cites the source GLSL line. A naga-backed
      `terrain_wgsl_parses_and_validates` test catches WGSL drift
      without a GPU.
      (3) `docs: ADR-043 (Unified terrain shader); amend ADR-036
      superseded` — full GLSL → WGSL line-mapping table, texture bind
      order (binding 0 → 12, all in Group 0), uniform-layout decoding
      of `flags.w` bits, 1×1 fallback policy, pre-applied
      `SMF_INTENSITY_MULT` reasoning, alternatives considered, and the
      list of items deferred to Sprints 26-36. ADR-036's status moves
      to "Accepted 2026-05-18; superseded by ADR-043 2026-05-21". The
      renderer-bar-parity ROADMAP's pre-renumbering reservation of
      ADR-038 for this work is documented; we took ADR-043 because
      ADR-038 was already claimed by Sprint 15's layered painter
      trio.
      (4) `render: Comet Catcher Remake parity fixture + manual-smoke
      README` — `crates/barme-app/src/parity_fixtures.rs` exposes
      `comet_catcher_fixture()` returning a `CometFixture { project,
      heightmap, splat }` shaped to the real BAR map's SMF header
      (16×12 SMU, heightmap 1025×769) + mapinfo (splat scales
      `{0.004, 0.007, 0.003, 0.0018}`, mults `{0.4, 0.4, 0.65, 0.9}`,
      sun_dir normalised from `(1.2, 0.92, -0.79)`, ambient
      `(0.55, 0.51, 0.51) × SMF_INTENSITY_MULT` pre-dimmed CPU-side,
      `splatDetailNormalDiffuseAlpha = 1`, the texture-presence
      bitfield set to `0b111`). 13 tests pin the fixture's shape.
      `assets/parity-fixtures/comet/README.md` documents the manual
      smoke procedure: capture BAR reference screenshots at 3 angles,
      load the fixture in the editor, eyeball-compare at 2-8 SMU.
      Heightmap is synthesised because `*.smf` is gitignored;
      Sprint 36 (parity-validation) ships an SMF parser alongside the
      mandatory headless-render harness.
      (5) `docs: SRS + ROADMAP rollup for Sprint 25 (R1 — terrain
      shader parity)` — this STATUS UPDATE.
      Tests: barme-app 337 → 350 (+13 fixture +
      `terrain_wgsl_parses_and_validates` + 5 new unit pins);
      barme-core 279 unchanged; barme-pipeline 213 unchanged.
      Workspace total 829 → 842. cargo fmt / clippy / test all green.
      **Renderer-parity arc: 1 / 8 done.** Sprint 26 = water polish
      (fresnel + foam + caustics + perlin + refraction + reflection —
      amends ADR-042). Subsequent arc sprints: 28 atmosphere + fog,
      29 features (S3O / 3DO), 30 shadows, 34 grass, 35 emission +
      sky-reflect + parallax, 36 parity validation + SRS §2.1 #11
      closeout.
- [x] **STATUS UPDATE 2026-05-21 (Sprint 26 / R3, ADR-044 — water
      polish, BumpWater port).** Amends ADR-042 by replacing the
      Sprint 14 flat alpha-blended water MVP with a port of
      `cont/base/springcontent/shaders/GLSL/BumpWaterFS.glsl` to
      `crates/barme-app/src/water.wgsl`. **8 commits on main**:
      (1) `render: planar reflection pass + mirrored-Y camera` —
      fixed 1024² `ReflectionTarget` (RGBA8 + Depth32Float), second
      terrain pipeline with `Face::Front` cull to compensate for the
      mirrored-Y winding, `OrbitCamera::view_proj_matrix_reflected_y0`
      with two unit tests pinning eye/target/up flip + above↔below-Y
      projection equivalence, `App.water_reflections: bool`
      (per-session, default ON).
      (2) `render: refraction copy + render-pass split, water samples
      refraction + reflection` — single offscreen pass splits into
      terrain → COPY (`offscreen.color → refraction_copy`) → water +
      lines + markers (LoadOp::Load). Water bind group expands from 1
      (uniform) to 5 (uniform + refraction tex/sampler + reflection
      tex/sampler) rebuilt on offscreen-RT resize via
      `RenderResources::rebind_water`. `WaterU` grows 96 → 176 B.
      WGSL gets a `naga::front::wgsl::parse_str` + validate test.
      (3) `render(water): fbm-driven surface normal via WGSL perlin` —
      Quilez 2D hash + quintic-smoothed bilerp + 4-octave fbm with
      3.0 lacunarity matches BumpWaterFS::GetNormal's four normalmap
      taps. Procedural (no GPL-2.0 `waterbump.png` vendoring).
      (4) `render(water): fresnel + foam + caustics composite` —
      Schlick fresnel with clamped `dot` (PITFALL #6 NaN guard),
      refraction-luma foam proxy (the coastmap bake is Sprint 27
      candidate), two-axis sine caustics gated on refraction luma.
      `WaterU` grows 176 → 192 B with `eye: vec4<f32>`.
      (5) `render(water): lava emission self-illumination` — gated
      branch when `water_mode ∈ {Lava, Magma}`; emission_color × (1 +
      caust · 0.5) × hardcoded 0.5 daylight (Sprint 28 plumbs the
      sun-dot ramp; Sprint 30 inhibits under cast shadows).
      (6) `ui(water): Polish section in inspector` — collapsible
      "Polish" below Flood: reflections toggle, fresnel min/max/power,
      reflection distortion, perlin start freq + lacunarity. Seven
      new `HelpId::Water*` variants with FINDINGS §1.5 default
      citations in the hover text.
      (7) `docs: ADR-044` — full decision record. Amends ADR-042;
      notes the prompt's ADR-039 vs actual ADR-044 numbering
      correction; documents the 8 load-bearing changes, alternatives
      (vendor BAR assets, ping-pong RTs, full-res reflection, sky
      cubemap, coastmap bake, schema lift for missing `WaterBlock`
      fields), consequences, and operational pitfalls for follow-up
      sprints (Face::Front cull discipline, two-uniform-buffers
      requirement, foam proxy edge cases, lava daylight + shadow
      wiring hooks for Sprints 28/30).
      (8) `docs: SRS + ROADMAP rollup + parity fixtures` — this
      STATUS UPDATE plus `assets/parity-fixtures/{coastlines,gecko,
      lava-sample}/README.md` shipping manual-smoke procedures
      anchored to the Ocean / Tropical / Lava preset baselines (the
      Sprint 36 ΔE harness will automate against these).
      Tests: barme-app 350 → 353 (+3: 2 reflection camera-math + 1
      water.wgsl naga validator); barme-core 279 unchanged;
      barme-pipeline 213 unchanged. cargo fmt / clippy / test green.
      **Renderer-parity arc: 2 / 8 done.** Sprint 27 = Inspector
      consistency refactor + brush-card lift (off-arc UX work).
      Next arc sprint is Sprint 28 (atmosphere + fog).
- [x] **STATUS UPDATE 2026-05-21 (Sprint 27 / U5 — Inspector
      consistency refactor).** Off-arc UX work; closes the
      three-sprint UI polish wave begun in Sprint 19 (tooltips) and
      Sprint 22 (onboarding). No new ADR — work fits inside ADR-030
      (Phase 3 layout shell) + ADR-035 (widget contract). **6
      commits on main**: (1) `ui(widgets): lift brush_card to
      widgets.rs` — extract `App::brush_card` into
      `widgets::brush_card(BrushCard)` with `label / icon / ring_color
      / active / hover_help: HelpId` descriptor; both Sculpt and
      PaintLayer call sites consume the same widget. (2)
      `ui(widgets): lift sticky_chip_strip` — extract the symmetry +
      map-size chip band from `App::inspector_sticky_chips` into
      `widgets::sticky_chip_strip(&[ChipDesc])`; the App method is
      now a thin wrapper that builds the two descriptors and
      delegates. (3) `ui(widgets): standardise row delete buttons` —
      new `widgets::row_delete_button(HelpId)` (16 px `Icon::X`,
      muted by default and `t.red` on hover) replaces five
      `small_button("×")` and one `small_button("delete group")`
      across metal / geo / feature / start position / ally group
      rows, plus the layers_panel layer delete; new
      `HelpId::LayerRowDelete` covers the formerly-hardcoded
      tooltip. (4) `ui(inspector): lift min_height to header` —
      Inspector header HEIGHTMAP section gains `Min height` next to
      `Max height` (new `HelpId::HeaderMinHeight` with the
      `World Y at raw 0 …` text); Water inspector's duplicate
      "Heightmap range" section retires. The Water FLOOD section's
      "Sea-floor depth" stays because it's the load-bearing flood-
      gesture input, not a redundant editor. (5) `ui(inspector):
      enforce one-accent-section-per-tool` — `inspector_feature`'s
      Category and Placed demoted from accent (Picker was already
      accent: true), so the tool reads with a single primary
      surface; new `tests/inspector_section_pattern.rs` audits the
      pattern via source-file parsing (slices `main.rs` into per-fn
      bodies, counts the 3rd positional `accent` arg of every
      `widgets::section` call, and asserts ≤1 accent: true per tool
      inspector). Companion test verifies the PaintLayer accent
      lives in `layers_panel.rs` and that every tool inspector calls
      `self.inspector_sticky_chips`. (6) `ui(inspector_select):
      wrap info in 'Mode' accent section` — Select now joins the
      canonical skeleton (sticky chip strip → one accent section);
      pattern audit tightened from ≤1 to exactly 1 accent per tool
      inspector and Select moved into the audit list.
      Tests: barme-app 353 → 356 (+3: inspector_section_pattern.rs);
      barme-core 279 unchanged; barme-pipeline 213 unchanged.
      `cargo fmt && cargo clippy --workspace --all-targets -- -D
      warnings && cargo test --workspace` green.
      **Renderer-parity arc unchanged at 2 / 8 done** (Sprint 27
      was off-arc). Next arc sprint is Sprint 28 (atmosphere + fog).
- [x] **STATUS UPDATE 2026-05-21 (Sprint 28 / R2, ADR-045 — atmosphere
      + fog).** Third renderer-parity step. The cheapest sprint on the
      arc as the prompt framed it ("most of the work is plumbing — the
      shader math is one fog equation"). **7 commits on main**:
      (1) `render(atmosphere): AtmosphereUniforms + bind group
      plumbing` — new 144 B `#[repr(C)]` block (9 × vec4: sun_color,
      sky_color, fog_color, fog_start_end, cloud_color, wind,
      sky_axis_angle, sun_dir, flags) bound at terrain group 0 /
      binding 13. `App::atmosphere_uniforms_for_render` populates each
      frame from `MapInfo.atmosphere` + `lighting`. `TerrainCallback`
      carries the new struct through `prepare()` (the `prepare()` site
      writes both terrain + reflection-pass bind groups against the
      same buffer — fog and sky don't change between the passes).
      (2) `render(atmosphere): exponential height fog in terrain
      fragment stage` — `smoothstep(fog_start, fog_end, dist_norm ×
      exp(-y × falloff))` then `mix(lit, fog_color, fog_t ×
      fog_color.a)`. Reuses §7's `to_eye` (WGSL flags redefinition;
      smoothstep clamps defensively against `fog_start == fog_end`).
      (3) `render(atmosphere): sun-colour angle ramp + cross-shader
      sun_dir` — `mix(fog_color, ground_diffuse, clamp(sun_dir.y, 0,
      1))` warms terrain at low sun; matching `daylight = pow(1 -
      clamp(sun_dir.y, 0, 1), 0.7)` in `water.wgsl` resolves Sprint
      26's hardcoded `0.5` placeholder. Atmosphere uniforms grew
      sun_dir (9th vec4 → 144 B); size-pin test updated. Water bind
      group adds a 5th binding to share the atmosphere buffer.
      (4) `render(atmosphere): sky-colour offscreen clear` — main
      offscreen `LoadOp::Clear` now reads `atmosphere.sky_color` per
      frame via `atmosphere_clear_color(&self.atmosphere)`. Pixels not
      covered by the terrain rasteriser show the project's configured
      sky tone instead of the legacy navy. Reflection-pass clear
      retains the navy fallback (over-painting the reflection RT with
      sky_color would mis-tint the water reflection sampler).
      (5) `render(atmosphere): wind direction from atmosphere block`
      — `water_draw_for_frame` reads wind from the deterministic
      `sin/cos` ramp; `water.wgsl` switches to `atmos.wind.xy` as the
      canonical wind source (single point of truth shared with terrain
      and the future grass shader). PITFALL #7 satisfied — no
      seed-controlled noise, parity fixtures reproduce byte-for-byte.
      (6) `docs(adr): ADR-045 atmosphere + fog` — the prompt's
      "ADR-040" number was already taken (Sprint 16 paint viewport);
      renumber to ADR-045. Documents seven decisions: sibling block
      (not extension), exponential fog math, sky-as-clear (not a
      dedicated pipeline), sun-angle ramp, deterministic wind, cubemap
      deferral, fog_start == fog_end defensiveness. Operational
      pitfalls for the deferred-cubemap sprint included.
      (7) `assets(parity): foggy-map + sunset fixtures` — two
      README-based parity fixtures matching the Sprint 26 lava-sample
      pattern. foggy-map exercises dense fog + altitude thinning;
      sunset exercises the warm-tinted ground at `sun_dir.y ≈ 0.2`
      plus the matching lava daylight brightening.

      **Skybox cubemap loading explicitly deferred** per user scope
      decision (2026-05-21) — the prompt called it "the heaviest
      engineering work" and the four shipped Sprint-28 effects (fog,
      sun ramp, sky-as-clear, wind) don't depend on it. The follow-up
      sprint lands `texture_cube<f32>` binding + PNG-folder loader +
      content-addressed cache + dedicated `sky.wgsl` pipeline +
      horizon gradient blend. `AtmosphereUniforms.flags[0] = has_skybox`
      stays at 0 until then.

      Tests: barme-app 356 → 360 (+4 atmosphere: size-pin,
      defaults-match-MapInfo, MapInfo-round-trip, wind determinism);
      barme-core 279 unchanged; barme-pipeline 213 unchanged. `cargo
      fmt && cargo clippy --workspace --all-targets -- -D warnings &&
      cargo test --workspace` green.

      **Renderer-parity arc: 3 / 8 done.** Next arc sprint is Sprint
      29 (feature asset decoding — S3O + decal sprites).
- [x] **STATUS UPDATE 2026-05-22 (Sprint 29 / R5, ADR-046 — feature
      decal sprite atlas, Phase A).** Fourth renderer-parity step.
      Closes the visual gap on placed features: where Sprint 13 /
      ADR-037 only emitted category-coded glyphs (filled circles for
      rocks, triangles for trees / geo vents, outline rings for
      props, filled-with-stroke for wreckage), Sprint 29 / Phase A
      adds a `MarkerShape::TexturedSprite { layer: u32 }` variant
      that samples a per-family diffuse from a new 32-layer
      `texture_2d_array<f32>` bound at `markers.wgsl @group(0)
      @binding(2)`. **8 commits on main**:

      (1) `assets(catalog): v3 catalog from upstream mapfeatures
      families` — v2's 34 synthetic names matched zero upstream
      featureDefs (`beyond-all-reason/mapfeatures @ 3b79163` +
      `Beyond-All-Reason @ 3763840` were both grepped). v3 lists
      ~85 representative variants across 21 families with a new
      `families` map carrying per-family diffuse path + source
      ("mapfeatures" / "bar" / "engine"). Per-entry `family: Option
      <String>` references the map. BREAKING change for any project
      using v2 synthetic names → those entries fall through to
      `FALLBACK_FEATURE_VISUAL`, matching engine behaviour on map
      load. User scope-decision 2026-05-21 picked this path over
      "additive" or "ship pipeline only" alternatives.

      (2) `deps: image[tga] + bcdec_rs workspace dep` — upstream
      diffuses ship as TGA; `image` needs the feature enabled.
      `bcdec_rs` lands now as the BC1/BC3/BC5 decoder so Sprint
      29b's S3O texture-ref resolver inherits a working integration
      point. Phase A doesn't exercise the DDS code path.

      (3) `scripts(decals): fetch-feature-decals.sh + tools/ ignore`
      — idempotent shell script clones / refreshes the user's local
      `~/code/Beyond-All-Reason/mapfeatures` and copies diffuses
      into `tools/feature-decals/<family>/diffuse.tga`. Pinned to
      the audited upstream commit with soft drift warnings. `--check`
      mode verifies state without copying. `--refresh` force-
      overwrites. Mirrors the existing `fetch-textures.sh` /
      `fetch-pymapconv.sh` pattern. License rationale per ADR-046:
      upstream has `AI_POLICY.md` but no `LICENSE` — the editor
      binary contains no redistributed content; the .sd7 output
      references features by name and the engine resolves textures
      at game-time. `tools/feature-decals/` is gitignored.

      (4) `app(decals): feature_decals module — TGA/PNG/BMP -> 128²
      RGBA8` — thin wrapper that decodes a feature decal diffuse
      and resizes to the fixed `SPRITE_SIZE = 128` consumed by the
      atlas. Dispatch by extension: TGA/PNG/BMP via the `image`
      crate; DDS returns `DecalError::DdsUnsupported` (typed error,
      not silent skip — the registry warns loudly on the gap).
      5 unit tests pin the size constants + fixture round-trip +
      each error branch.

      (5) `render(markers): TexturedSprite shape + decal texture
      array` — `MarkerShape::TexturedSprite { layer: u32 }` adds
      shape_id 5; `MarkerInstanceGpu::texture_layer: u32` consumes
      one slot of the former 3-u32 pad (struct stays 48 B / 16 B-
      aligned). `markers.wgsl` Instance mirror updated; fragment
      shader `case 5u:` samples `decal_atlas` at LOD 0 with
      `tex_uv = (uv + 1) × 0.5` and Y-flip (image-crate output is
      top-row-first; quad uv.y = +1 points up on screen).
      `MarkerResources` gains a 32-layer 128² Rgba8UnormSrgb
      texture + linear-clamp sampler + bind group entries 2 + 3.
      `write_decal_layer` is the host-side upload API consumed by
      the registry. Memory: 2 MB.

      (6) `app(decals): FeatureDecalRegistry wires v3 catalog
      families to GPU atlas` — `FeatureCatalog` parses the v3
      `families` map; `FamilyDef::decal_layer: Option<u32>` is the
      runtime slot. `populate_decal_registry` scans
      `tools/feature-decals/<family>/diffuse.tga` for every family
      with `diffuse_texture` set, decodes, uploads, and stamps the
      assigned layer. `resolved_visual()` prefers `TexturedSprite`
      when the entry's family has a populated layer (radius 3× the
      category glyph); fallback to the existing glyph path
      otherwise. Removes the staged dead-code allows from the
      Sprint 29 / R5 staged landing.

      (7) `app(inspector): 32x32 thumbnail per family in F7 picker`
      — `FamilyDef::egui_thumbnail: Option<TextureHandle>`
      populated alongside `decal_layer` from the same decoded RGBA
      buffer; the same egui `Context::load_texture` call that fills
      it lives inside `populate_decal_registry` so one decode pass
      fills both the GPU atlas and the egui texture cache.
      `inspector_feature`'s picker row gains a horizontal slot
      with the thumbnail (or a category-tinted glyph fallback) to
      the left of display/name labels. Custom `Debug` impl on
      `FamilyDef` since `egui::TextureHandle` isn't Debug.

      (8) `assets(parity): feature-zoo fixture` — 33-feature 4-SMU
      smoke fixture exercising both the TexturedSprite path (16
      vendored mapfeatures families) and the glyph fallback path
      (5 families lacking upstream diffuse: kapok / rocks30 /
      tombstone / xmascomwreck / geovent). README-based manual
      smoke procedure until Sprint 36's parity-validation harness
      automates ΔE comparison.

      **Phase B (S3O parsing + 3D thumbnail render passes)
      explicitly deferred to Sprint 29b** per user scope decision
      2026-05-21. Phase B needs a new `barme-render-s3o` crate
      (parser + thumbnail render pass + content-addressed cache);
      the kickoff brief draft lives at the end of
      `devlog/sprint-29-feature-asset-decoding/`.

      Tests: barme-app 360 → 365 (+5 feature_decals + 2 marker;
      lint test pinned to v3 catalog names); barme-core 279
      unchanged; barme-pipeline 213 unchanged. `cargo fmt && cargo
      clippy --workspace --all-targets -- -D warnings && cargo test
      --workspace` green.

      **Renderer-parity arc: 4 / 8 done.** Next arc sprint is
      Sprint 30 (directional shadows); Sprint 29b for Phase B
      feature meshes / thumbnails is the feature-side follow-up.
- [x] **STATUS UPDATE 2026-05-22 (Sprint 29b / R5, ADR-047 — feature
      thumbnail bake from S3O, Phase B).** Fifth renderer-parity
      step. Closes the per-variant visual gap left by Phase A: where
      Sprint 29 / ADR-046 only stamped per-FAMILY diffuse decals (so
      `pdrock1` and `pdrock5` looked identical in the F7 picker and
      the viewport), Sprint 29b bakes per-ENTRY 128² thumbnails
      from the upstream `.s3o` mesh — each variant now shows its
      actual geometry. **9 commits on main**:

      (1) `crates(s3o): scaffold barme-render-s3o` — new leaf
      crate hosts the parser / thumbnail bake / cache. Types +
      stubs only so downstream wiring (catalog v3.1, registry
      rewrite) lands in parallel.

      (2) `render(s3o): binary parser port` — port of
      `RecoilEngine/rts/Rendering/Models/S3OParser.cpp` covering
      header read (52 B), recursive piece tree walk with offset
      accumulation, and triangulation of triangle-strip / quad
      primitives matching the engine's `Trianglize()` semantics
      byte-for-byte. 7 unit tests + an opt-in upstream-fixture
      test (auto-skipped when `~/code/Beyond-All-Reason/
      mapfeatures` isn't cloned).

      (3) `render(s3o): CPU rasteriser for thumbnail bake` —
      top-down ortho 128² rasteriser with bilinear-filtered
      diffuse, two-sided Lambert lighting (NdotL clamped to
      `[0.35, 1.0]`), depth test, pre-multiplied-alpha output.
      CPU not wgpu so cache keys are deterministic across
      drivers and there's no async device handshake. 5 unit
      tests including a real upstream pedro1.s3o bake.

      (4) `render(s3o): content-addressed thumbnail cache` —
      `$XDG_CACHE_HOME/barme/feature_thumbnails/<sha>.png` with
      `sha256(s3o_bytes)` as key. Atomic publish via
      `NamedTempFile + persist`. 6 unit tests including
      round-trip and wrong-dimensions rejection.

      (5) `assets(catalog): v3.1 schema` — adds
      `families.<key>.s3o_pattern` (template like
      `"ad0_banyan/{name}.s3o"`) and per-entry `s3o` (literal
      override for the 9 entries where the entry name doesn't
      match the .s3o filename — agorm_rock and cycas families).
      Read only by the fetch script; runtime registry checks
      `tools/feature-s3o/<entry.name>.s3o` directly.

      (6) `scripts(s3o): fetch-feature-s3o.sh + tools/ ignore`
      — mirrors `fetch-feature-decals.sh`'s pattern. 85 entries
      hardcoded; first-run on a fresh checkout installs
      ~3.2 MB. `--check` / `--refresh` / `BARME_MAPFEATURES_DIR`
      env supported. License rationale matches ADR-046:
      upstream has no LICENSE; `tools/feature-s3o/` is
      gitignored.

      (7) `render(markers): bump MARKER_DECAL_LAYERS 32 → 128`
      — atlas memory 2 MB → 8 MB. Inside PITFALLS §1 budget.
      Fits the 85 Phase B entries plus headroom (~27 layers).

      (8) `app(decals): populate_decal_registry bakes per-entry
      S3O thumbnails` — the big integration. Two-pass walk:
      Phase B per-entry first (read .s3o → sha → cache hit OR
      parse + bake_thumbnail + cache store → upload to atlas +
      build egui handle), Phase A per-family second for any
      family with `diffuse_texture` whose entries aren't all
      Phase-B-covered. `resolved_visual` + `thumbnail_for` walk
      entry → family → category glyph → unknown fallback.
      `CatalogEntry` gains `decal_layer` + `egui_thumbnail`
      runtime slots (serde-skipped, with a custom Debug impl
      since `egui::TextureHandle` lacks one). Synthetic
      mid-grey diffuse fallback for families with
      `diffuse_texture: null` (kapok) — silhouette + Lambert
      still produces a recognisable thumbnail.

      (9) `docs(adr): ADR-047 + SRS/ROADMAP rollup` — this
      block + the feature-zoo README addendum + ADR-047 in
      `docs/DECISIONS.md` documenting the six decisions
      (CPU rasteriser, catalog v3.1, per-entry baking, content-
      addressed cache, atlas bump, new leaf crate).

      **Mid-sprint dev ergonomics commit (not numbered):**
      `scripts(dev): cargo wrapper sources rustup env then exec
      cargo` — user-requested helper that eliminates the
      `. "$HOME/.cargo/env" && cargo ...` boilerplate. All
      cargo invocations now go through `./scripts/dev.sh`.

      Tests: barme-render-s3o introduces 20 new tests (parser,
      thumbnail bake, cache). barme-app unchanged at 365;
      barme-core 279 unchanged; barme-pipeline 213 unchanged.
      `cargo fmt && cargo clippy --workspace --all-targets --
      -D warnings && cargo test --workspace` green.

      Phase C (BAR-side feature .s3o vendoring for rocks30 /
      tombstone / xmascomwreck / geovent) is the natural next
      feature-asset sprint when the user wants those 8 entries
      to also render as 3D thumbnails. Today they stay on Phase
      A's category-glyph fallback (visible + correctly placed
      in the viewport, just not 3D).

      **Renderer-parity arc: 5 / 8 done.** Next arc sprint is
      Sprint 30 (directional shadows).
- [x] **STATUS UPDATE 2026-05-22 (Sprint 31 / U4 — toast queue +
      confirmation modal).** Closes the 2026-05-20 UX audit's
      finding #4: the editor's single `App.last_error:
      Option<String>` slot in the status strip is replaced by a
      proper notification queue (Info/Warn/Error with
      auto-dismiss + coalesce + cap) and the destructive-action
      paths (delete ally group, delete layer, new/open project
      while dirty) pick up a confirmation modal primitive. **6
      commits on `main`**:

      (1) `ui(toast): ToastQueue + App helpers` — new
      `crates/barme-app/src/ui/toast.rs` ships `Toast`,
      `ToastQueue`, `ToastKind` (Info / Warning / Error),
      `ToastAction` (`OpenLintPanel` / `OpenBuildLog` /
      `OpenHelpArticle` / `DismissMigrationToast`) +
      `App::toast_info / toast_warn / toast_error /
      toast_with_action`. Render via `egui::Area` anchored
      bottom-right, non-blocking; 500 ms fade-out tail; hard
      cap at 10 with oldest-non-error eviction; 5 s dedup-
      coalesce. 13 unit tests.

      (2) `ui(toast): retire last_error; migrate 12 sites` —
      drops `App::last_error: Option<String>`. Every call site
      (save / open / heightmap load failures, texture import
      and downsample, GC outcomes, build-gated-by-lint with
      `OpenLintPanel` action) now spawns through the matching
      toast helper. Status-strip's red `last_error` line
      replaced by a tone-coloured count chip
      ("N notifications" — green / amber / red by worst
      tone). Lint-gate test migrated to assert on the toast
      queue + the OpenLintPanel action.

      (3) `ui(confirm): modal primitive + intent dispatch` —
      new `crates/barme-app/src/ui/confirm.rs` ships
      `ConfirmDialog` / `ConfirmResult` / `confirm_modal()`.
      Fullscreen click-eating backdrop at
      `egui::Order::Foreground`, centred 360 px dialog, Esc
      cancels + Enter confirms (other keys pass through —
      PITFALL #4), destructive flag tints the confirm button
      `t.red`. App gains `pending_confirm` +
      `pending_confirm_intent` + a `ConfirmIntent` enum
      (DeleteAllyGroup / DeleteLayer /
      NewProjectDiscardingChanges /
      OpenProjectDiscardingChanges /
      OpenPathDiscardingChanges). 6 unit tests.

      (4) `ui(confirm): wire destructive paths` — the four
      destructive paths now route through the modal. Empty /
      clean cases bypass (delete an ally group with zero
      positions, delete a layer that's never been painted into).
      Sprint-17 migration toast lifts from its bespoke
      `egui::Window` to the new toast queue with a persistent
      Info entry + DismissMigrationToast action — per-project
      `Project.migration_toast_dismissed` flag preserved (PITFALL
      #1). Sprint-20's save-before-build stub picks up the
      backdrop + Esc-cancel (kept its 3-button shape).
      `LayerMask::has_painted_tiles()` added to `barme-core`
      with 2 tests.

      (5) `ui(toast): surface build + lint events as toasts` —
      `poll_build_state` toasts on the terminal transition:
      Done → info(name + duration); Failed → error_with_action
      (OpenBuildLog); Cancelled → warn(duration). `recompute_lint`
      fires a single Warning + OpenLintPanel action on the
      `0 → >0` hard-error edge; steady-state errors stay quiet;
      re-fires coalesce via the dedup window into the existing
      entry's count. 1 new test.

      (6) `docs(srs+roadmap): rollup` — this STATUS UPDATE +
      the SRS §2.1 mirror.

      Test counts: barme-app 364 → 397 (+33, of which 22 are
      new), barme-core 279 → 281. `cargo fmt && cargo clippy
      --workspace --all-targets -- -D warnings && cargo test
      --workspace` green. **Out of scope**: programmatic toast
      dismissal from worker threads (visual-only surface; the
      worker uses tracing for diagnostics), persistent toast
      log across editor sessions (the queue is in-memory),
      notification sounds, swipe-to-dismiss, undo of toast
      actions (the underlying `ProjectDiff` handles undo —
      modal + toast are transient state).

      Next off-arc UX sprint is Sprint 32 (F12 Launch in BAR
      + autosave); the renderer-parity arc resumes at Sprint
      34 (grass).
- [x] **STATUS UPDATE 2026-06-18 (Sprint 33 / T6 — ADR-049).** NFR CI
      gates. The repo had no `.github/` at all; the 2026-05-20 audit
      flagged ~6 NFR commitments with no test gate. Now in place:
      (1) `rust-toolchain.toml` (dev stable 1.96) + a `[stable, "1.90"]`
      MSRV `cargo check` matrix (`RUSTUP_TOOLCHAIN` per-cell so the toml
      doesn't override the MSRV cell); (2) criterion benches
      `brush_latency` + `procgen_apply` on a `perf` lane (>1.5× fails)
      plus CI-safe ceiling tests — **NFR-Performance** honoured (16-SMU
      smooth ≈ 1 ms); (3) **NFR-Determinism** honoured — fixed a silent
      break where `sd7::package()` stored per-file mtimes (no
      timestamp-strip flag despite the SRS promise); added
      `-mtm- -mtc- -mta-` + a 7z-only CI test + an `#[ignore]`d
      end-to-end test; (4) **NFR-Portability** honoured — a 3-OS release
      matrix + Linux AppImage (`scripts/build-appimage.sh`), and fixed a
      latent blocker where `repo_root()` used the compile-time
      `CARGO_MANIFEST_DIR` (packaged binaries now resolve via
      `BARME_ROOT`); macOS ships **experimental** (unsigned). Headless
      wgpu via Mesa Lavapipe; `ci_without_gpu` cfg registered; failing
      CI uploads test logs. New: `CONTRIBUTING.md`, `README.md`. Commits
      `ci:`/`bench:`/`pipeline(sd7):`/`ci(release):`/`release(appimage):`
      under `Sprint 33 / T6`. `cargo fmt && cargo clippy --workspace
      --all-targets -- -D warnings && cargo test --workspace` green on
      Linux. **Caveat:** Sprint 33's stated prerequisite — Sprint 32
      (F12 + autosave / NFR-Crash safety) — is **not in the repo** (F12
      is a disabled stub, no autosave). The CI gates are independent of
      that feature work. The renderer-parity arc resumes at **Sprint 34
      (grass rendering)**.
- [x] **STATUS UPDATE 2026-06-18 (Sprint 34 / R6 — ADR-050 — grass
      rendering).** Seventh renderer-parity step; the renderer-parity
      arc is now **7 / 8 done**. Grass renders as instanced
      camera-billboard quads on the terrain, swaying in the wind and
      receiving shadows. New `barme-core::grass` (CPU density bake — a
      logistic slope falloff → one coverage byte per heightmap texel;
      persists to `<project>/.barme-cache/grass-density.png`; type-0
      mask uniform until F15), new `barme-app::grass` (deterministic
      per-blade scatter over a 16-elmo turf grid; `fmix32`-hashed
      jitter keyed on the turf cell so blades don't shimmer; two-pass
      global scale holds the 100k-blade Vega 8 budget; 200-elmo LOD),
      new `grass.wgsl` (billboard vertex + wind sway sharing the
      atmosphere wind + `water_time_seconds` with water, 3×3 PCF shadow
      RECEIVE, leaf-edge taper, smooth LOD fade). Schema gap closed
      (house rule #1): `maxStrawsPerTurf` + `bladeWaveScale` added to
      `GrassBlock` (were missing vs engine `ReadGrass`). `View > Grass`
      toggle + `Grass density` slider (iGPU throttle). 8 commits
      (schema → density → scatter → shader+pipeline → app wiring → ADR
      +fixture → rollup); +12 tests (barme-app 397→404, barme-core +4,
      barme-pipeline +1). `cargo fmt && cargo clippy --workspace
      --all-targets -- -D warnings && cargo test --workspace` green.
      New fixture `assets/parity-fixtures/grass-field/`. **Validation
      boundary:** CPU bake / scatter / WGSL / GPU-layout unit-tested in
      headless CI; live visual match + the 100k-blade <4 ms Vega 8
      budget need a GPU session (hardware-pending; devlog
      `sprint-34-grass-rendering/`). The prompt called this "ADR-043"
      but that's taken; grass ships as ADR-050. Next arc sprint:
      Sprint 35 (emission + sky-reflect + parallax).
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
- [x] **C8** "Lint My Map" pass — Sprint 21 ships `barme-pipeline::lint`
      with a 31-variant `LintRule` registry covering every silent
      failure mode in PITFALLS.md §1–§28 plus FINDINGS §12 / NEW-1..NEW-10.
      10 hard errors gate the Build button; 18 warnings + 3 info-tier
      surface in the lint panel without blocking. Sprint 19's panel
      stub gets a real body grouped by severity, Fix buttons that
      dispatch through `ProjectDiff::EditMapInfo` for undo, and F9
      form per-tab dots that route by `field_path` prefix. App's
      `validation_summary` heuristics retire — chip tone aggregates
      from the registry every frame.
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
