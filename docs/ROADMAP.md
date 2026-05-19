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
- [ ] **F4** Texture painting via DNTS splat channels (4 RGBA, ≤4 splat textures)
      — **D1 shipped 2026-05-18** (ADR-025 / ADR-027): 16-slot CC0
      ambientCG starter palette + `scripts/fetch-textures.sh` (sha256
      pinned, idempotent, `--check` HEAD-probe). Per-slot layout under
      `tools/textures/<NN-slot>/{diffuse.png, normal.png, meta.toml}`
      is the contract D3's registry depends on. F4 itself remains
      gated on D2 (DNTS bake, ADR-026), D3 (`barme-core::splat`),
      D4 (fragment shader blend), D5 (splat tool inspector), D6
      (mapinfo emission + `.sd7` bundling).
- [ ] **F5** Metal-spot placement (point + radius → red-channel density)
- [ ] **F6** Geo-vent placement
- [ ] **F7** Feature placement (trees, rocks, wreckage) into a Lua gadget
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
