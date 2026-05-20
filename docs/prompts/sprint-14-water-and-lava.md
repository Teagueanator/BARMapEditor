# Sprint 14 — Water + Lava (C9): schema emission + Tool::Water + Inspector + presets + flat preview plane

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 14** — **C9** — a one-sprint slice that gives the editor a
first-class water / lava authoring path. After this sprint, a user can:
1. Pick a water preset (Ocean / Tropical / Acid / Lava / Magma / None / Custom)
   and ship a `.sd7` whose `mapinfo.water` block actually controls how BAR
   renders / damages water.
2. See a translucent water plane at Y=0 in the editor's 3D preview, tinted
   by the chosen preset, so they can visualize coastline coverage live.
3. Carve the heightmap below Y=0 with a one-click depth (the existing
   `Brush::Lower` wired through `Tool::Water`) — this is the actual
   workflow for "flood this region" since BAR's water level is a global
   constant (see Phase 1 below).

**Source-of-truth reading:** before doing anything, read
`/home/teague/code/BARMapEditor/devlog/research-water-lava/logs/2026-05-19T10-59-15__water-lava-engine-research.md`
end-to-end. The report has all the BAR engine citations, sample-map data,
proposed UX, and slice plan. This prompt assumes that report's findings;
don't re-derive them.

**Prerequisites:**
- Sprint 12 (C6 + D6) MUST be ticked. D6's `mapinfo.resources` emit path
  is what C9 leans on for textures (water uses `resources.water_texture` /
  `foam_texture` etc.); also Sprint 12 reserves the staging-path
  conventions C9 mirrors.
- **Sprint 13 (renderer-depth rework, ADR-037) MUST be ticked.** C9's
  Slice 2 (flat water plane) needs the offscreen RT + depth attachment +
  alpha-blend order Sprint 13 sets up. Without it, the water plane will
  z-fight the terrain at Y=0 and depth-fail under markers.
- Sprints 1–13 done.

**Out of scope:**
- **Foam / fresnel / caustics / wave perlin / refractions** — those are
  the renderer-parity arc's "Water" sub-sprint (sketched at line 550-552
  of `sprint-13-renderer-depth-rework.md` as a future Sprint-20+ item).
  Sprint 14 ships a flat colored quad with alpha; the polish lives
  downstream.
- **`lightEmissionTex`-driven lava glow at night.** That's its own
  `resources` sub-emission gated on Sprint 12 / D6 wiring; gets picked up
  in a future renderer-parity sub-sprint. Sprint 14 ships the Lava /
  Magma preset's `damage` + `surfaceColor` but not the emission map.
- **Per-channel damage curves** ("shallow safe / deep instakill"). BAR
  has one global `damage` float; the community feature ask is real but
  engine-unsupported. Out of scope (Stage 3 wishlist).
- **Multi-water-level maps.** Engine forbids (water plane is `consteval`).
  Not even attempting.
- **Water tab inside the F9 form editor (Sprint 18).** That work stays in
  Sprint 18 — it becomes the "advanced / raw fields" backstop now that the
  dedicated Water tool has a first-class form. Sprint 18 was already
  drafted; only its positioning shifts (see "Cross-sprint coordination"
  below).

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (mapinfo schema —
   specifically the `water` sub-block), §2.1 #6 (`splatDetailNormalTex`
   + water LOS bug), §2.1 #8 (DNTS + water LOS — relevant lint), §3.2
   F8 / F9 (form editor — Sprint 18 owns the raw-fields backstop).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §6
   (`voidWater` + `planeColor` mutual exclusion), §8 (DNTS + water LOS
   TV-snow bug), §15 (subtable forms).
4. **THE PRIMARY DOCUMENT:**
   `/home/teague/code/BARMapEditor/devlog/research-water-lava/logs/2026-05-19T10-59-15__water-lava-engine-research.md`
   — engine citations + sample-map data + UX proposal + slice plan.
   Trust this for all engine semantics; the prompt below maps the
   slices to concrete code.
5. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs:372-687`
   — `WaterBlock` (already complete, 30+ Option fields),
   `bar_default_with_water()` (exists but currently dead code — Sprint 14
   wires it).
6. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/mapinfo.rs:108-134, 252-299`
   — water emit. `water_block()` writes every field; the top-level
   gating is `info.water.is_some()`.
7. `/home/teague/code/BARMapEditor/crates/barme-core/src/project.rs`
   — `Project`. Sprint 14 adds `water_mode` + `water_overrides` +
   (optionally) a `WaterPatch` newtype. Migration from legacy:
   `min_height < 0` → default `WaterMode::Ocean`; else `WaterMode::None`.
8. `/home/teague/code/BARMapEditor/crates/barme-core/src/undo.rs` —
   `ProjectDiff` patterns. Sprint 14 adds `SetWaterMode { from, to }`
   and `EditWaterField { field_path, from, to }` (B5 / ADR-033).
9. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs` —
   `Tool` enum (currently 7 variants), `App` struct, central viewport
   pointer dispatch, `inspector_*` functions. Sprint 14 adds an 8th
   `Tool::Water` and a new `inspector_water()`.
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/icons.rs` —
    extend with `Icon::Water` (wave glyph, line-drawn — match the
    existing style: two sine arcs at ~30° offset).
11. `/home/teague/code/BARMapEditor/crates/barme-app/src/terrain.wgsl`
    — fragment shader. Sprint 14 adds a second draw call (or a new
    `water.wgsl`) for the alpha-blended water plane.
12. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs` —
    bind groups. Sprint 14 adds a `WaterUniforms` struct + one bind
    group entry.
13. `/home/teague/code/BARMapEditor/crates/barme-core/src/brushes/`
    — `Brush::Lower` already exists from Sprint 1-3. Sprint 14's
    Tool::Water dispatches it with a different ephemeral depth.
14. `/home/teague/code/BARMapEditor/docs/prompts/sprint-18-minimap-and-form-editor.md`
    — context for the form editor's Water tab. Sprint 14's data path
    becomes the source of truth; Sprint 18's tab is the "advanced /
    raw 30 fields" disclosure.
15. `/home/teague/code/BARMapEditor/docs/prompts/sprint-19-lint-pass.md`
    — verify the four water lints are in the catalogue:
    `VoidWaterWithPlaneColor`, `TidalStrengthWithoutWaterSurfaceColor`,
    `TerrainBelowZeroWithoutWater`, and the new
    `WaterModeSetWithoutTerrainBelowZero` (add to Sprint 19 if absent).
    The report also flags `DntsWithWater` (PITFALL §8) and
    `LavaWithoutEmissionTexture` (deferred until emission tex ships).
16. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md`
    — C9 entry under Stream C. Confirm ADR-042 reservation.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-water-data-and-emission
./devlog/log.sh new stage-1-water-tool-and-inspector
./devlog/log.sh new stage-1-water-preview-plane
```

Three sub-items: data, UI, render.

## Step 3 — Scope

Six commits on `main`:

### Commit 1 — Slice 1: Schema + emission + migration

- `crates/barme-core/src/water_presets.rs` (new): hard-coded `WaterPreset`
  values for each mode. Each preset is a fully-populated `WaterBlock`
  literal copied from a real BAR map (cite the source map in the doc-
  comment for each preset).

  ```rust
  pub enum WaterMode {
      None,      // emits no water sub-table; forceRendering = false
      Ocean,     // Coastlines Dry baseline
      Tropical,  // Gecko Isle Remake
      Acid,      // Acidic Quarry (damage=200, yellow-green)
      Lava,      // synthesized (damage=1000, orange-red)
      Magma,     // synthesized (damage=5000, deep red; just under hover-block 1e4)
      Custom,    // user's overrides preserved verbatim
  }

  pub fn preset_water_block(mode: WaterMode) -> Option<WaterBlock>;
  ```

- `crates/barme-core/src/project.rs`:
  - Add fields:
    ```rust
    /// Active water preset. `#[serde(default)]` so old projects load
    /// as `WaterMode::None`; `Project::after_load_migrate` re-derives:
    /// `min_height < 0` → `WaterMode::Ocean`, else `WaterMode::None`.
    #[serde(default)]
    pub water_mode: WaterMode,

    /// Sparse user overrides on top of the preset. All fields
    /// `Option<…>` (same shape as `WaterBlock`). `None` everywhere
    /// = "use the preset as-is". Persisted across mode changes
    /// (switching presets does NOT blow away overrides — switching
    /// to `Custom` lets them all bleed through).
    #[serde(default)]
    pub water_overrides: WaterBlock,
    ```
  - `Project::after_load_migrate` extension: when `water_mode` is at
    default (`None`) AND `min_height < 0`, set to `Ocean`. **Run only
    on first load of pre-Sprint-14 projects** — gate on a `migration_v`
    bump or on `water_mode == default && min_height < 0`.

- `crates/barme-core/src/mapinfo_schema.rs::From<&Project> for MapInfo`
  (line 758):
  - Replace `bar_default()` with a flow that:
    1. Calls `bar_default()` for the baseline.
    2. If `project.water_mode != WaterMode::None`, calls
       `preset_water_block(project.water_mode)` to get the preset
       `WaterBlock`, then merges `project.water_overrides` over it
       (per-field: `override.field.or(preset.field)`).
    3. Sets `info.water = Some(merged)`.
    4. For `WaterMode::None` AND `project.min_height < 0`, lint the
       inconsistency in Sprint 19's pass — but the emit still produces
       no water sub-table (today's behaviour).
  - Honor the `voidWater` ↔ `plane_color` mutual exclusion: if
    `info.void_water == true`, force `merged.plane_color = None`. Log
    `warn!` and surface in the validation chip.

- `crates/barme-core/src/undo.rs`:
  - `ProjectDiff::SetWaterMode { from: WaterMode, to: WaterMode }`.
  - `ProjectDiff::EditWaterField { field: WaterField, from: WaterValue, to: WaterValue }`
    where `WaterField` is a small enum identifying which of the ~30
    fields the edit hit (Surface RGB / Plane RGB / Damage / ...).
    `WaterValue` is a tagged union of `f32` / `[f32; 3]` / `[f32; 4]`
    / `bool` to cover the field types.
  - Apply/revert + round-trip tests.

- Tests:
  - Each preset round-trips through emit + reparse byte-equal (within
    Lua-formatting tolerance).
  - `WaterMode::None` + `min_height < 0` → `info.water.is_none()` but
    Sprint 19 lints (a deferred test added in Sprint 19's commit).
  - `WaterMode::Ocean` + overrides on `damage = 30` →
    emitted `water = { damage = 30, surfaceColor = {...Ocean's...}, ... }`.
  - `void_water = true` + `WaterMode::Ocean` → `plane_color` is
    cleared, `warn!` fires.
  - Migration: pre-Sprint-14 fixture with `min_height = -120` → load →
    `water_mode == Ocean`. Same fixture re-saved + reloaded → still
    `Ocean` (no re-migration). Identical fixture with `min_height = 50`
    → `water_mode == None`.

**After Commit 1 ships:** the editor builds correct water blocks. BAR
loads the map and treats `min_height < 0` regions as water with the
correct preset behavior. No UI yet — the user has to flip `water_mode`
in code or accept the load-migration default.

### Commit 2 — Slice 2 (MVP): flat water plane in 3D preview

The renderer-parity arc owns the polished water plane (fresnel + foam +
caustics + lava emission). Sprint 14 ships the MVP: one alpha-blended
quad at Y=0 across the full map extent, tinted by the active preset's
`surface_color`. This alone makes the feature self-explanatory.

- `crates/barme-app/src/water.wgsl` (new): a tiny vertex + fragment
  shader.
  - Vertex: emits the four corner positions of the map's XZ extent at
    Y=0. (Quad — two triangles.)
  - Fragment: outputs `(surface_color, surface_alpha)`.
- `crates/barme-app/src/render.rs`:
  - Add `WaterResources { pipeline, vertex_buf, uniform_buf, bgl }`.
  - `WaterUniforms { surface_color: [f32; 4], plane_y: f32, alpha: f32, _pad: f32 }`.
  - Pipeline:
    - **Blend mode**: alpha-over (`src.rgb * src.a + dst.rgb * (1 - src.a)`).
    - **Depth state**: depth test ENABLED, depth write DISABLED
      (translucent — Sprint 13's depth-rework rules apply).
    - **Cull**: back-face. Quad winding matches a top-down view.
  - Draw order: AFTER terrain (so terrain depth gates water — terrain
    above Y=0 occludes the plane).
- `crates/barme-app/src/terrain.wgsl`: no changes (the water draw is a
  separate pipeline).
- `App::render`:
  - When `project.water_mode != WaterMode::None`, dispatch the water
    pipeline after the terrain pass. Compute `surface_color` from
    `preset.merge(overrides).surface_color.unwrap_or(BAR_DEFAULT)`.
    Alpha from `surface_alpha.unwrap_or(0.1)`.
  - Cross-tool ghosting (the B1 pattern from canvas-symmetry sprint):
    when `Tool::Water` is active, full opacity (1.0×); otherwise
    50 %× to indicate "you're not editing this right now."
- Tests:
  - Headless render: spawn the wgpu pipeline, draw one frame with
    `WaterMode::Acid`, sample the centre pixel of an off-map quad
    region, assert RGB ≈ `(0.65, 0.8, 0.1)` ± 5/255 (Acidic Quarry
    surface_color). The headless harness from earlier renderer sprints
    is the model.
  - Toggle `WaterMode::None` ↔ `Ocean` → frame pixel changes from
    "terrain-only" to "terrain + tint" within one frame.

### Commit 3 — Slice 3: `Tool::Water` + Inspector form skeleton

- `crates/barme-app/src/main.rs`:
  - Add `Tool::Water` variant. Slot it before `Procgen` in `Tool::ALL`
    (so the strip order is Sculpt / Splat / Metal / Geo / Features /
    Water / Procgen / Select).
  - Keyboard `W`. Icon `Icon::Water`. Label "Water / Lava".
  - Central-viewport pointer dispatch: LMB-drag → call
    `Brush::Lower::apply(...)` with `depth = App::water_carve_depth`
    (default `-80.0` elmos, ephemeral session state). RMB raises (mirrors
    Sculpt). No splat / metal / geo interactions.
  - When `Tool::Water` active: the brush ring's color is tinted by the
    preset's `surface_color` (so the user sees "I'm flooding acid here").
- `inspector_water(&mut self, ui)` (new): see §4.3 of the research
  report for the visual layout. Sections:
  - **Preset** — 7 chips: None / Ocean / Tropical / Acid / Lava / Magma /
    Custom. Click → `ProjectDiff::SetWaterMode { from, to }`. Active
    chip highlighted (accent stroke).
  - **Behaviour** — `damage` slider (range `0..=10_000`, log-style
    label; tooltip explains 1e3 = ground blocked, 1e4 = hover blocked),
    `void_water` pill toggle (clears plane_color if enabled — confirm
    via toast), `tidal_strength` slider (range `0..=30` — `tidal_strength`
    lives at the MapInfo top level, not inside `water`, but the UI shows
    it here because it's conceptually water-related).
  - **Appearance** — `surface_color` color picker, `plane_color` color
    picker (disabled when `void_water = true`), `surface_alpha` ramp
    slider, `wave_size` (perlin amplitude) slider, `foam_strength`
    (waveFoamIntensity) slider.
  - **Flood** — `carve_depth` DragValue (ephemeral session field on
    `App`, NOT persisted; default `-80 elmos`), `min_height_clamp`
    display (read from `Project.min_height`, with an "Auto-set
    from heightmap min" button that scans the current heightmap and
    sets `Project.min_height = min(0, observed_min)`).
  - **Advanced (collapsing)** — placeholder section. Tooltip:
    "Full 30-field form available under Mapinfo → Water tab
    (Sprint 18 / F9)."

- Every edit in the inspector emits a `ProjectDiff::EditWaterField`
  for undo.

### Commit 4 — Slice 4: presets and lava-atmosphere link

- Each preset chip is a one-click apply. Clicking a non-Custom chip:
  1. Emits `ProjectDiff::SetWaterMode { from, to }`.
  2. Leaves `water_overrides` UNTOUCHED (user tweaks persist across
     preset changes, which is the photoshop-like behaviour the report
     argues for).
  3. For **Lava** + **Magma**: surface a one-frame confirmation toast
     "Apply Magma atmosphere preset too? (red fog, dim sun)". Yes →
     emit a second `ProjectDiff::SetAtmosphereField` batch with the
     lava-atmosphere values (extends an existing atmosphere preset
     enum or, if absent, hard-codes the deltas here). No → just the
     water change.
- **Lava-atmosphere values** (hard-coded, cite the research report):
  ```rust
  fog_color  = [0.9, 0.3, 0.1]      // red-orange
  sun_color  = [1.0, 0.5, 0.3]      // dim warm
  fog_start  = 0.1
  fog_end    = 1.5
  cloud_color = [0.4, 0.2, 0.15]
  cloud_density = 0.7
  ```
- "Custom" preset: unlock the Advanced disclosure (in Commit 3 this is
  a placeholder; Commit 4 doesn't expand it — the actual full form is
  Sprint 18's responsibility). The chip label changes from "Custom" to
  "Custom (N overrides)" with N being the count of non-None fields in
  `Project.water_overrides`.

### Commit 5 — Validation chip + cross-tool ghosting

- `App::validation_summary` extension:
  - WARN when `water_mode == None && min_height < 0` (BAR will render
    the engine's default flat blue ocean instead of nothing — surprise).
    Wording: "Your terrain dips below Y=0 but no water preset is
    selected. BAR will render default ocean."
  - WARN when `water_mode != None && min_height >= 0` (`forceRendering`
    needed). Wording: "Water preset set but terrain never dips below
    Y=0. Enable forceRendering or carve a basin."
  - WARN on DNTS + water LOS bug (PITFALL §8): "Map has DNTS slots and
    water — known engine TV-snow bug when LOS widgets run. Test in BAR
    before shipping."
  - These three are also surfaced by Sprint 19's lint pass; Sprint 14
    just wires them to the always-on chip so the user sees them while
    editing.

- Cross-tool ghosting: when `Tool::Water` inactive, the water plane
  renders at 50 % the configured `surface_alpha` so it's visible but
  doesn't dominate. Implementation: an `App::water_tool_active: bool`
  bound into `WaterUniforms.alpha_scale`. Test that a frame rendered
  with `Tool::Sculpt` shows the plane fainter than `Tool::Water`.

### Commit 6 — Rollup

- STATUS UPDATEs in SRS / ROADMAP:
  - F-something water (likely F? — check the SRS feature table for the
    canonical id; if absent, request a SRS edit appending "Water / Lava
    map property + tool" as a new F-row in C9's commit message).
  - ROADMAP STATUS UPDATE noting C9 closes the "water emission gap"
    (`From<&Project>` never populated water).
- Tick the C9 checkbox in `phase-3-plan.md`.
- ADR-042 in `docs/DECISIONS.md`: water preset architecture, mutual-
  exclusion rules, the "data path is source of truth; renderer-parity
  arc polishes the visual later" boundary, the cross-tool ghosting
  rule. Cite the research report.
- Close the three devlog folders.

## Step 4 — Standing constraints

Same as Sprint 13. `cargo fmt && cargo clippy --workspace
--all-targets -- -D warnings && cargo test --workspace` green
before every commit.

Tracing: `info!` on preset change + `Tool::Water` activation;
`debug!` on per-frame water plane render; `warn!` on void_water /
plane_color collision auto-resolve + DNTS+water lint trip.

## Step 5 — Out of scope (loud)

- **Polished water rendering** — fresnel, foam, caustics, perlin wave
  motion, lava emission glow. All renderer-parity-arc work, multiple
  future sub-sprints. Sprint 14 ships flat-color alpha; the renderer
  parity arc layers on top.
- **Multi-water-zone maps.** Engine-impossible (`consteval`). Surface
  the limitation in the Inspector tooltip; don't attempt.
- **F9 Water tab in the form editor.** Sprint 18. Sprint 14's Inspector
  is the primary entry point; Sprint 18 will become the
  raw-30-fields advanced backstop.
- **`light_emission_tex` for lava-glow textures.** Future renderer-parity
  sub-sprint, gated on Sprint 12's `resources` emit being extended.
  Sprint 14's Lava preset sets `damage` + `surface_color` but does NOT
  reference an emission texture.
- **Atmosphere preset cross-link** beyond the Lava/Magma toast. The
  atmosphere preset system itself might not exist yet (check
  `barme-core::atmosphere`); if it doesn't, hard-code the deltas
  inline. Don't refactor the atmosphere system in this sprint.
- **Per-tile damage / shaped lava** — out of engine scope.

## Step 6 — Critical pitfalls (read twice)

1. **`voidWater` + `planeColor` mutual exclusion** (PITFALL §6).
   Toggling `void_water = true` MUST clear `plane_color` before the
   merge into `MapInfo`. Test this explicitly. The auto-resolve fires
   a `warn!` so the user sees it; the validation chip surfaces it.
2. **Water plane Y=0 is NOT configurable.** The engine has
   `GetWaterPlaneLevel()` as `consteval 0.0f`. Any UI affordance that
   even suggests "lake at height 50" is misleading — don't add one.
   The "Flood" section explains: water = wherever heightmap < 0.
3. **Carve brush is `Brush::Lower`, NOT a new brush.** Sprint 14 reuses
   the existing trait + symmetry fan + undo stack. The only new code
   is the App-level dispatch when `Tool::Water` is active: pull
   `water_carve_depth` from session state.
4. **Damage thresholds** (Sim/MoveTypes/MoveDefHandler.cpp:81-160):
   `damage ≥ 1e3` blocks ground units; `≥ 1e4` blocks hovers. The
   Lava preset ships at `1000` (just at the ground-block boundary —
   the report cites this) and Magma at `5000` (deeply blocking ground;
   hovers can still strategically cross). DO NOT silently land
   anything ≥ 1e4 because hover gameplay is BAR-central.
5. **`tidal_strength` lives at MapInfo top-level, NOT inside `water`.**
   Sprint 14's Inspector surfaces it under the Water section for UX
   reasons, but the data field is `MapInfo.tidal_strength`, not
   `WaterBlock.tidal_strength`. Don't move the schema.
6. **Migration runs ONCE per project.** The pre-Sprint-14 fixture
   migration (`min_height < 0` → `WaterMode::Ocean`) must NOT re-fire
   on subsequent loads of the same project. Gate on
   `water_mode == WaterMode::default() && min_height < 0` AND a
   migration version bump (`Project.schema_v: u32` or similar — check
   how prior migrations did it).
7. **Forward-compat `WaterMode` enum.** Add `#[serde(other)]` or a
   custom deserialize so a future preset (e.g., "Geyser") in a newer
   project loads on an older editor as `Custom` — never crashes.
8. **DNTS + water LOS bug** (PITFALL §8). The lint fires WARN not GATE;
   Beherith's forum post describes it as a TV-snow artifact when a
   LOS widget runs over DNTS terrain with water. The lint surfaces but
   does NOT block emit.
9. **Alpha-blend depth state** (Sprint 13 dependency). Depth TEST on,
   depth WRITE off — translucent geometry rule. Forgetting depth-test
   makes the water plane render in front of terrain that should
   occlude it.
10. **Carve depth in elmos, NOT in heightmap units.** The `Brush::Lower`
    contract is in elmos (the existing heightmap brushes match this).
    Sprint 14's carve depth widget reads `f32 elmos`, default `-80.0`.
11. **Quad extent matches `MapSize::elmo_extents()`.** The water plane
    vertex buffer is regenerated when map size changes (e.g., wizard
    creates a new project at a different SMU). Test the resize.
12. **Order of preset → override merge: `override.field.or(preset.field)`.**
    Per-field, never wholesale override. Switching preset reuses
    overrides on un-set fields and substitutes the new preset
    elsewhere — matches Photoshop's "I tweak the slider once, it
    persists across preset changes."

## Step 7 — Cross-sprint coordination

- **Sprint 12** (running in parallel): its splat-pipeline work doesn't
  touch the water block. No conflict.
- **Sprint 13** (renderer-depth): Sprint 14's water plane reuses the
  offscreen RT + depth attachment Sprint 13 set up. Sprint 13's prompt
  already mentions water at line 550 as a future renderer-parity-arc
  item; Sprint 14 ships the MVP, the arc polishes later.
- **Sprints 15-17** (layered painter): no conflict. `Tool::Water` and
  `Tool::PaintLayer` are independent. The Layers panel UI (Sprint 17)
  doesn't intersect the water Inspector.
- **Sprint 18** (minimap + form editor): the F9 Water tab there
  becomes the "raw 30-fields advanced backstop". Sprint 18's prompt
  must be edited (separate task — already on the list) to note that
  Tool::Water is the primary entry point and the form tab is the
  power-user disclosure. The form tab still ships from Sprint 18 — it
  reuses Sprint 14's `Project.water_overrides` model.
- **Sprint 19** (lint pass): Sprint 14's validation chip fires the
  same lints Sprint 19's pass will surface in a richer panel. Sprint 19
  must confirm `VoidWaterWithPlaneColor`,
  `TidalStrengthWithoutWaterSurfaceColor`, `TerrainBelowZeroWithoutWater`,
  `WaterModeSetWithoutTerrainBelowZero`, `DntsWithWater`,
  `LavaWithoutEmissionTexture` (the last deferred until emission tex
  ships) are in its catalogue.

## Step 8 — Exit criteria

- 6 commits on `main`: data + emission, water plane render MVP,
  Tool::Water + inspector, presets + lava-atmosphere link, validation
  chip + cross-tool ghosting, rollup.
- 3 devlog folders filled.
- 1 checkbox ticked in `phase-3-plan.md` (C9).
- ADR-042 in `docs/DECISIONS.md`. Cite the research report.
- SRS / ROADMAP STATUS UPDATEs (water / lava feature shipped, polished
  rendering deferred to renderer-parity arc, form editor's Water tab
  repositioned to power-user backstop per Sprint 18 prompt).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
  && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Launch editor on a fresh project (wizard with `min_height = -120`).
    Open project → `Project.water_mode` migrates to `Ocean` (one-time).
  - Hit `W` → tool strip highlights "Water / Lava", inspector swaps,
    3D viewport shows a translucent blue plane at Y=0 over the lower
    regions of the heightmap.
  - Switch preset to "Acid" → plane re-tints yellow-green within one
    frame. Carve a basin with LMB-drag → the basin floods green.
  - Switch to "Lava" → toast appears asking about Magma atmosphere.
    Accept → fog turns red-orange. Plane is now orange-red.
  - Tweak `surface_alpha` slider → plane translucency updates live.
    Save → reload → preset + overrides round-trip.
  - Switch tool to Sculpt → water plane drops to 50 % alpha
    (cross-tool ghosting). Switch back to Water → full alpha.
  - Toggle `void_water = true` while a `plane_color` override is set
    → toast "plane_color cleared because voidWater is on";
    validation chip shows the resolution.
  - Build a `.sd7` → mapinfo.lua contains `water = { damage = 1000,
    surfaceColor = {1.0, 0.4, 0.1}, ... }`. Load in BAR → ground
    units take damage in flooded regions; visual matches the editor.
  - Pre-Sprint-14 project (no water_mode in file) → loads, migrates
    once, saves, reloads → migration does NOT re-fire (gated on
    `migration_v` bump).
  - `cargo test --workspace -- water` runs all new tests green.
- Final devlog log summarising: "Water + lava authoring shipped.
  Renderer polish (foam / fresnel / caustics / emission) deferred to
  renderer-parity arc. Sprint 18 picks up the F9 advanced-fields tab."

Start by running `git status`, then re-read the research report
section §4.2-§4.7 — the UX shape it lays out is what Commit 3
implements. Begin with Commit 1 (data + emission); Commit 2's render
plane is independent and can run in parallel if you fork the work.
