# Sprint 18 — F10 minimap auto-gen + F9 mapinfo form editor (D7, C7)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 18** — **D7** (F10 minimap auto-generation from the
textured terrain view) and **C7** (F9 mapinfo form editor — schema-
driven UI that lets the user edit every mapinfo field, plus a "Raw
Lua" sanity-check tab).

After this sprint, every F1–F11 MVP item except F12 (Launch in BAR —
deferred to Sprint 32) is feature-complete. The user can iterate a
project's lighting, atmosphere, water, terrain types, etc. through
a form, and the auto-generated minimap reflects whatever the layer
stack composites.

**Prerequisites (all complete):**
- Sprints 1–17 done. F4 closed end-to-end via the layered painter trio
  (Sprints 15-17 / ADR-038 / ADR-039 / ADR-040 / ADR-041).
- Sprint 10 (mapinfo audit fix) shipped: `sundir`/`sunDir` dual-emit,
  `skyAxisAngle`, `voidAlphaMin`, `sunDir.w = 1.0`. The F9 form
  surfaces all of these.
- Sprint 13 (renderer depth rework, ADR-037) shipped: offscreen RT +
  depth attachment. D7's headless minimap render reuses this pattern.
- Sprint 14 (water + lava, ADR-042) shipped. The F9 form's Water tab
  is a **power-user backstop** for the dedicated `Tool::Water`
  Inspector that Sprint 14 ships; do not duplicate the discoverable
  preset UX.

**Note on Sprint 17 followups (NOT in scope here — tracked for Sprint 23):**
The 16-SMU `Tool::PaintLayer` OOM, orphan-imported-texture GC on
layer-delete-undo, and the legacy `SplatConfig`-on-disk retirement
are explicitly **deferred to Sprint 23** (Sprint-17 cleanup). Do not
attempt to fix them here. If you trip over the OOM during your smoke
run, document it in your devlog and continue.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.2 F9 / F10 (the
   product asks); §1.2 (minimap = always 1024×1024 inside SMF, with
   8-mip-level chain); §1.3 (mapinfo subtables — what the form has
   to cover); §2.1 #11 reframed by the renderer-parity arc — the
   minimap reflects the editor's render, which is now an approximation
   of BAR's but Sprint 18 is NOT a renderer-parity sprint; the minimap
   will get higher-fidelity once Sprint 25 (terrain shader parity)
   lands.
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §4 (heightmap
   dim `64·N + 1`), §11–21 (all audit corrections — the F9 form must
   round-trip these without data loss), §23 (start-position
   wrapping shape), §27 / §28 (look_at_lh + GetWaterPlaneLevel
   consteval — informational, the minimap is top-down so the sign
   bug doesn't bite).
4. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — the C1+C3+Sprint 10 schema. Every `pub` field gets a form
   row. 9 sub-blocks (smf, lighting, atmosphere, water, resources,
   splats, terrainTypes, grass, teams) + top-level fields.
   The schema is the source of truth — the form is a direct
   reflection.
5. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/mapinfo.rs`
   — the Lua emitter (Sprint 5 / C2 / ADR-029, Sprint 10 audit-fixed).
   The form's "Raw Lua" tab calls `render_mapinfo(&info)` and displays
   the result read-only. Output must be byte-stable.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/widgets.rs`
   (from ADR-035) — `section / chip / ramp_slider_labelled /
   pill_toggle / split_button / key_combo / icon_button /
   slot_picker_grid`. The form uses these heavily. **DO NOT**
   invent new widget primitives unless absolutely necessary.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs`
   — the terrain pipeline + `OffscreenTarget` (ADR-037). D7's
   headless minimap render uses the same pipeline.
8. `/home/teague/code/BARMapEditor/crates/barme-core/src/project.rs`
   — `Project::SCHEMA_V`, `after_load_migrate`, `Project.layers`
   (now the source of truth for diffuse — `splat_config` is
   `#[serde(skip_serializing)]` post-Sprint 17).
9. `/home/teague/code/BARMapEditor/crates/barme-core/src/layers/mod.rs`
   — `LayerStack::bake_diffuse` (Sprint 15) and
   `LayerStack::dnts_layers()` (Sprint 17). The minimap renders
   through the live `compositeRT` (Sprint 16) so it reflects what
   the painter has produced.
10. `/home/teague/code/BARMapEditor/docs/DECISIONS.md` — search for
    ADR-017 (heightmap GPU upload pattern), ADR-028 (mapinfo schema
    model), ADR-029 (three-file emission), ADR-035 (UI widget
    contract), ADR-037 (offscreen RT + depth), ADR-038 / ADR-039 /
    ADR-040 / ADR-041 (layered painter quartet), ADR-042 (water tool).

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-f10-minimap
./devlog/log.sh new stage-1-f9-mapinfo-form
```

Use the per-item folders so the two items have independent log streams.

## Step 3 — Scope

In order, one commit per item:

### 1. D7 — F10 minimap auto-generation

- **Module** (`crates/barme-pipeline/src/minimap.rs`, new):
  ```rust
  pub fn render_minimap(
      project: &Project,
      slot_resolver: &dyn SlotResolver,
      out_png: &Path,
  ) -> Result<()>;
  ```
  - Allocates a 1024×1024 RGBA8 offscreen render target (matches
    SRS §1.2: minimap is ALWAYS 1024² inside SMF, with 8-mip-level
    chain).
  - Configures an orthographic camera looking down +Y, framing the
    full map (`(0, 0)..(smu_x * 512, smu_z * 512)` in elmos).
  - Renders ONE pass through the existing terrain pipeline
    (`crates/barme-app/src/terrain.wgsl`) with:
    - heightmap from `Project.heightmap`
    - composited diffuse from `LayerStack::bake_diffuse` (CPU; same
      path the `.sd7` build uses — keeps minimap and final diffuse
      visually identical)
    - DNTS slot textures from `tools/textures/<slot>/diffuse.*` for
      slots referenced by `LayerStack::dnts_layers()` (re-using D5's
      slot-thumbnail cache where possible)
    - sun direction from `mapinfo.lighting.sun_dir`
    - editor-preview simplifications: no shadows, no water-
      absorption, no atmospheric scatter (Sprint 25+ adds those;
      minimap is intentionally cheap)
  - Reads back via `queue.submit() + buffer.map()` (NOT a blocking
    `read_buffer` — use an async future via
    `pollster::block_on` since this is a build-time function not
    a frame-time one).
  - Writes the 1024² RGB to `out_png` via the `image` crate.
  - Total time budget: ~500 ms on a Vega 8 iGPU (vs the editor's
    ~16 ms frame budget; this is a one-shot build-time call so
    perf is not critical).

- **Override** (`Project.minimap_override: Option<PathBuf>` — new
  field, requires `Project::SCHEMA_V` bump to 2 + migration entry):
  - If set, skip `render_minimap()` and copy the override PNG into
    staging.
  - The PNG must be 1024×1024 — validate at load time and error
    with a clear message if not.

- **Staging wiring** (`crates/barme-pipeline/src/sd7.rs`):
  - Call `render_minimap()` (or copy the override) into
    `staging/maps/<projectname>.png` BEFORE PyMapConv.
  - Pass it to PyMapConv via the existing `-mini` flag. **Verify
    the PyMapConv flag name** by reading
    `tools/pymapconv/src/pymapconv.py` — the upstream CLI surface
    is captured in `devlog/stage-0-validation/logs/2026-05-17T16-57-48__pymapconv-vendoring.md`.

- **Headless device setup** (`crates/barme-pipeline/src/headless_render.rs`,
  new): set up a one-off wgpu device for the minimap pass. Sharing
  the editor's `Device` is fragile across Win/macOS contexts. Use
  `wgpu::Instance::new(...) → Device::request(...)` with no surface.
  - **Platform-portability checklist** (mandatory):
    - Configure `wgpu::Backends::PRIMARY` (Vulkan on Linux/Windows,
      Metal on macOS, D3D12 fallback on Windows). Do **not** hard-code
      `wgpu::Backends::VULKAN` — that breaks macOS.
    - On Linux CI without a GPU, fall back to `wgpu::Backends::GL`
      via Mesa software rendering (slow but works). Document via a
      `MINIMAP_BACKEND` env var override (default = PRIMARY).
    - Test on at least one of macOS / Windows before merging — or
      document the limitation in the devlog and gate the CI test
      behind a `#[cfg_attr(not(unix), ignore)]`.

- **F10 surface in the F9 form**: the form gains a "Minimap" tab
  showing the auto-generated image (or override preview), with a
  file-picker button to set / clear the override. **C7 handles
  the UI; D7 only ships the rendering.**

- **Unit tests** (`crates/barme-pipeline/tests/minimap.rs`, new):
  - `minimap::tests::render_default_project_succeeds` — runs the
    pipeline against a default 8-SMU project, asserts the output
    PNG is 1024² RGBA.
  - `minimap::tests::override_passthrough` — `minimap_override`
    set to a fixture 1024² PNG → that PNG is copied unchanged.
  - `minimap::tests::override_wrong_dim_errors` — fixture 512²
    PNG → error.

- **Touch points**:
  - `crates/barme-pipeline/src/minimap.rs` (new).
  - `crates/barme-pipeline/src/headless_render.rs` (new).
  - `crates/barme-pipeline/src/sd7.rs` (wire the call).
  - `crates/barme-core/src/project.rs` (`minimap_override` +
    `SCHEMA_V = 2` + migration step).
  - `crates/barme-app/src/launcher.rs` (`build_and_install` calls
    through the new minimap path).

- **No new ADR** — D7 reuses ADR-017 (heightmap upload), ADR-037
  (offscreen RT). The minimap's render path is a strict subset of
  the editor's preview.

### 2. C7 — F9 mapinfo form editor

- **Inspector route**: F9 is NOT a `Tool::MapInfo` variant.
  Instead, add a button to the top action bar:
  `widgets::icon_button(Icon::MapInfo)` opening a modal-ish
  `egui::Window`. The form lives outside the tool/inspector
  cycle because mapinfo edits are non-modal — the user might
  want to tweak gravity while painting splats.

- **Form tabs** (`egui::Layout::top_down`, tab strip up top):
  - **General** — name, shortname, description, author, version,
    modtype (read-only, always 3), depend (read-only, always
    `{"Map Helper v1"}`).
  - **Map** — gravity, tidal_strength, maphardness, max_metal,
    extractor_radius, void_water, void_ground, **void_alpha_min**
    (only when `void_ground = true`), auto_show_metal.
  - **SMF** — min_height, max_height, smt_file_name_0 (read-only,
    auto-set from project name + sanitize via `Project::sanitize_name`).
  - **Lighting** — sun_dir (`[f32; 4]` editor: 4 `DragValue`s,
    W locked to 1.0 with a "reset W" button — see pitfall #3),
    ground/unit ambient/diffuse/specular colours
    (`color_edit_button_rgb`), ground/unit shadow density,
    specular_exponent.
  - **Atmosphere** — minWind / maxWind / fogStart / fogEnd /
    fogColor / sunColor / skyColor / **skyAxisAngle**
    (`[f32; 4]` — axis xyz + radians angle; layout: 3 axis
    components on one row + 1 angle component with degree-display
    option), skyBox (text input), cloudDensity / cloudColor.
  - **Water** — **advanced / raw 30-fields backstop** for the
    dedicated `Tool::Water` Inspector shipped in Sprint 14 / C9.
    Sprint 14 owns the discoverable entry point (preset chips,
    behaviour, appearance, flood); this tab is the power-user
    disclosure for fields the dedicated Inspector intentionally
    elides. Drives the same `Project.water_overrides`
    (`WaterBlock` sparse-Option overlay) that Sprint 14 set up —
    NOT a separate data path. Sections: full surface / plane /
    base / min colours, fresnel min/max/power, perlin params,
    wind speed, wave length, foam params, caustics paths,
    texture overrides. Top of the tab shows the active preset
    name + a "Reset to preset" button that clears all overrides
    for the active mode. Read-only display of `Project.water_mode`
    (changes go through the dedicated tool). Once Sprint 26 (water
    polish) lands, this tab gains additional fields automatically.
  - **Resources** — read-only when D6 / Sprint 17's layer stack
    drives the splatDetailNormalTex / splatDistrTex fields. Surface
    detailTex / specularTex / detailNormalTex / lightEmissionTex
    / skyReflectModTex / parallaxHeightTex as user-editable
    string fields (file pickers? defer to v2; text input for now).
  - **Splats** — **read-only summary** of the layer stack's DNTS
    bindings (Sprint 17 retired the editable per-channel splat UI).
    Lists each DNTS-bound layer's slot + channel + tex_scale +
    tex_mult. Edit path is "Open Layers panel" (the
    `Tool::PaintLayer` strip lives there).
  - **Terrain types** — table of `TerrainType` rows (name /
    hardness / receive_tracks / per-locomotor speeds). Add /
    remove rows. **F15** (type-map editor — deferred to a future
    Stage-2 sprint) extends this; for Sprint 18 the table is just
    name/hardness/per-locomotor-speed scalars, no painting.
  - **Custom** — free-form key/value table.
    `Project.mapinfo_overrides: HashMap<String, toml::Value>`.
  - **Raw Lua** — read-only `TextEdit::multiline` rendering
    `render_mapinfo(&MapInfo::from(&project))`. Reactive: re-
    renders whenever any other tab changes. Useful for the user
    to sanity-check before Build.
  - **Minimap** — preview of the auto-generated minimap + file-
    picker for `Project.minimap_override`. Calls D7's
    `render_minimap()` on a 1-Hz debounce when any field that
    affects the minimap (lighting / layers / heightmap) changes.

- **Form generation pattern**:
  - **NOT proc-macro-driven**. Manual form code is fine for a
    one-off schema. A proc-macro would add maintenance overhead
    for one client.
  - Each tab is a function: `fn lighting_tab(ui: &mut egui::Ui,
    info: &mut MapInfo) -> bool` where return value = true if
    any field mutated. Mutations flow through
    `ProjectDiff::EditMapInfo(MapInfoPatch)` for undo (B5).

- **Round-trip invariant**: edit a field in the form → struct
  updates → re-render Raw Lua tab → diff is exactly the edited
  field. If the form drops a field on round-trip, the user loses
  data silently. **Test**: pin a fixture `mapinfo.lua`,
  deserialize via `From<&Project> for MapInfo`'s inverse (or via
  C2's emitter parsed back), edit a single field, re-serialize,
  assert the only diff is the edited field. Adopt the
  `serde::Value` round-trip pattern if the typed schema can't do
  this directly.

- **F9 surface for the lint chips** (gates on Sprint 21 / C8):
  - The bottom status strip's validation-chip area gains a
    coloured dot in the F9 form's tab strip when ≥1 lint issue
    lives in that tab. Hovering shows a tooltip; clicking jumps
    to the field. This wiring is **partly speculative — implement
    the dot rendering now** (read from a `validation_summary`
    ad-hoc struct) and let Sprint 21 populate the struct with
    real lint output. **Sprint 19 lands the status-chip click
    affordance** that opens the lint-panel placeholder; the F9
    tab dots integrate with the same `App::lint_summary` field.

- **Tooltip coverage** (gates on Sprint 19 / U1): EVERY DragValue,
  ComboBox, color button, and text input in the form gets an
  `.on_hover_text()` calling out (a) what the field does, (b)
  units/range, (c) `mapinfo.lua` consequence. Sprint 19 ships a
  general tooltip catalogue — coordinate field naming if your
  schedule overlaps; if Sprint 19 hasn't shipped, write the
  tooltips inline and Sprint 19 will harvest them into the
  catalogue.

- **Unit tests** (`crates/barme-app/src/main.rs::tests` or
  `crates/barme-app/src/ui/inspector_mapinfo.rs::tests`):
  - `inspector_mapinfo::tests::round_trip_no_data_loss` — described
    above.
  - `inspector_mapinfo::tests::all_schema_fields_have_a_form_row`
    — uses Rust's compile-time reflection (manual exhaustiveness
    check). Use the schema struct's field count vs the form
    function's match-arm count; bump both together when adding
    fields. Per pitfall #1 below.
  - `inspector_mapinfo::tests::sky_axis_angle_round_trip` — pin
    the new `skyAxisAngle = [1, 0, 0, 1.5708]` case.

- **Touch points**:
  - `crates/barme-app/src/ui/inspector_mapinfo.rs` (new — one
    file with tab functions; OK to split into submodules if
    >600 LoC).
  - `crates/barme-app/src/main.rs` — top-bar `MapInfo` button
    opening the window; field-mutation routing through
    `ProjectDiff`.
  - `crates/barme-core/src/undo.rs` — new
    `ProjectDiff::EditMapInfo(MapInfoPatch)`. `MapInfoPatch` is
    an enum covering all leaf fields the form mutates.
  - `crates/barme-core/src/mapinfo_schema.rs` — `MapInfoPatch`
    enum lives here near the schema.

- **No new ADR** — F9 form is UI-only; uses ADR-028's schema +
  ADR-029's emitter + ADR-035's widget library.

### 3. Rollup commit

- STATUS UPDATEs in SRS / ROADMAP (F9 + F10 ticked).
- closing devlog logs for both items.
- "Sprint 19 = UI tooltip + help-text pass + status-chip wiring +
  validation-chip click affordance" handoff note. Sprint 19 audits
  EVERY existing widget across the app — your D7/C7 form is part
  of that audit; ship a first pass at tooltips and Sprint 19 will
  unify them.

## Step 4 — Standing constraints

Same as prior sprints. Devlog folder per item.

Tracing: `info!` on minimap-render start/end with timing; `trace!`
on staging dir population; `warn!` on dim-validation failures
(override 512² etc.); `error!` on PyMapConv flag-discovery failures.

## Step 5 — Out of scope

- **F12 Launch in BAR** — deferred to Sprint 32 (gated on Recoil
  `--map` invocation pattern + autosave being in place).
- **Sprint 21 lint panel population** — the panel itself ships
  here as a stub; rules come in Sprint 21.
- **F13 decompile import** — Stage 2 / future sprint.
- **Proc-macro form generation** — manual is fine here.
- **Splat-tab editing** — Sprint 17 retired the editable splat
  inspector; the new F9 Splats tab is read-only, surfacing what
  the Layers panel has set.
- **Sprint 23 painter cleanup** — orphan-texture GC + 16-SMU OOM
  root-cause are not this sprint's job.

## Step 6 — Critical pitfalls (read twice)

1. **Form round-trip is the entire game**: if a `MapInfo` field
   doesn't have a form row, edits to it via the Raw Lua tab can't
   be saved back to the typed schema. The `all_schema_fields_have_
   a_form_row` test catches drift. When Sprint 21 adds a lint
   rule, the lint module touches the schema; this test is the
   coupling.

2. **D7's minimap render at build time uses a HEADLESS wgpu
   device**. Don't share the editor's swapchain-bound device —
   doing so works on Linux but the macOS / Windows code paths
   risk Metal/D3D12 context contention. Use
   `wgpu::Instance::new(...) → Device::request(...)` with no
   surface. Test on at least one of macOS / Windows before
   merging (or document the limitation).

3. **The `sun_dir` editor is a `[f32; 4]`** since Sprint 10 fixed
   the schema to keep W=1.0 (engine intensity scalar, not
   `sunStartDistance`). The form must show all 4 components
   with W locked-to-1.0 by default. Editing W manually is
   power-user only; provide a "reset W = 1.0" affordance.

4. **`extractor_radius`** DragValue range 16..=200 (per Sprint
   11 / C4 convention). The engine default is 500 but BAR
   overrides to 80; setting it to 500 silently breaks mex snap.
   Surface a warning tooltip when value > 200; Sprint 21 lints
   at 500.

5. **`skyAxisAngle` is a `[f32; 4]`** (axis xyz + radians angle).
   Form layout: 3 axis components on one row + 1 angle component
   (with a degree-display option). Default `[0, 0, 1, 0]`.

6. **PyMapConv flag for the minimap PNG**: read
   `tools/pymapconv/src/pymapconv.py` directly. Don't trust
   stale wiki documentation. The flag is likely `-mini <path>` but
   verify before merging. Test with a fixture project.

7. **Headless wgpu adapter selection on CI**: GitHub Actions
   runners typically lack a GPU. The minimap render in CI either
   uses `wgpu::Backends::GL` with Mesa software rendering (slow
   but works), or the test is gated behind a `#[cfg_attr(not(unix), ignore)]`
   attribute and runs locally only. Document the choice in the
   devlog.

8. **F9 form's "lint chip dot" wiring uses an
   `App::lint_summary`** struct shared with Sprint 21. For Sprint
   18, populate it with a stub of zero issues; Sprint 21 fills it.
   Don't introduce a circular dep between `barme-app` (form) and
   `barme-pipeline` (lint) — the lint result is a plain
   `Vec<LintIssue>` the App computes on demand from the project
   state.

9. **`Project.minimap_override` bumps `SCHEMA_V` from 1 → 2.** Add
   a migration step in `Project::after_load_migrate` that sets
   `minimap_override = None` on v=1 loads. Test the migration
   with a v=1 fixture.

10. **The minimap render path reads `LayerStack::bake_diffuse`
    output**. On a brand-new project with an empty layer stack,
    `bake_diffuse` returns the `synth_biome_bmp` fallback (Sprint
    15). The minimap reflects that fallback — which is the same
    thing the `.sd7` build will ship if the user hasn't painted.
    Sprint 6 / B8 seeds a demo layer; verify the minimap of a
    fresh wizard project shows the demo's textured surface, not
    a grey square.

11. **Platform portability**: see the headless render section's
    checklist. WGSL is automatically cross-compiled to Vulkan
    SPIR-V (Linux/Windows), Metal MSL (macOS), and DXIL
    (Windows D3D12). The minimap pass must NOT use any
    backend-specific extensions; standard WGSL only.

## Step 7 — Exit criteria

- 3 commits on `main`: D7, C7, rollup.
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (F9 + F10 shipped).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Build a default project → `.sd7` contains a 1024² minimap PNG
    that visually matches the editor's top-down view (including
    the painted layer stack if the user added one).
  - Open F9 form → 12 tabs visible (General / Map / SMF /
    Lighting / Atmosphere / Water / Resources / Splats / Terrain
    types / Custom / Raw Lua / Minimap).
  - Edit `gravity = 200`, save, re-open project → form shows
    200. Raw Lua tab shows `gravity = 200`. Build → `mapinfo.lua`
    contains `gravity = 200`.
  - Edit `extractor_radius = 500` → form shows a warning tooltip.
    Sprint 21 will flip this to a lint chip.
  - Edit `sky_axis_angle = [1, 0, 0, 1.5708]` (90° around X) →
    raw Lua tab updates; rebuild loads cleanly in BAR.
  - Set `minimap_override = fixtures/test_minimap.png` (1024²) →
    Build → that PNG ships unchanged.
  - All schema fields have a form row (test passes).
  - Migration smoke: load a v=1 `.barmeproj` fixture → form opens
    without panicking; `minimap_override = None` after migration.
- Final devlog log summarising what shipped + "Sprint 19 = UI
  tooltip + help-text pass" handoff note. Note the F9 form's
  inline tooltips should match Sprint 19's catalogue conventions.

Start by reading the schema (`mapinfo_schema.rs`) end-to-end —
the form structure mirrors the struct layout 1:1. Then look at
`render.rs` to plan D7's headless-pass setup; it's the heavier
of the two items.
