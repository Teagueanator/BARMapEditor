# Sprint 12 — F7 features + splat .sd7 wiring (C6, D6)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 12** — **C6** (F7 feature placement with the stock
mapfeatures manifest) and **D6** (splat pipeline wiring — bake DNTS
per active slot, bundle the splat distribution PNG, populate
`mapinfo.resources` fields). After this sprint, painting splats in the
editor produces a `.sd7` that visibly textures in BAR — closing the
"painted distribution does NOT yet flow into the `.sd7`" gap left at
the end of Sprint 9.

**Prerequisites:**
- Sprints 1–6 done.
- Sprint 7 (D1 — texture pack), Sprint 8 (D2 + D3 — bake pipeline +
  splat module), and Sprint 9 (D4 + D5 — shader + UI) MUST be ticked.
  D6 consumes D1's `tools/textures/` registry, D2's
  `bake_dnts()` function, D3's `SplatDistribution`, and D5's
  `Project.splat_config`.
- Sprint 10 (mapinfo audit fix) **strongly recommended** — D6 writes
  into the splat block of `MapInfo`, and the source-audit corrections
  must be applied before the emitter ships these new fields.
  Specifically, D6 uses the SUBTABLE form of
  `splatDetailNormalTex` (PITFALL §15), which is the modern path the
  audit calls out.
- Sprint 11 (C4 + C5) is independent of C6/D6 but mostly done by now;
  if not, F6 geo-vent emission lands a `geovent` feature into the
  same `mapconfig/featureplacer/features.lua` C6 populates — touch
  the file carefully if Sprint 11 is concurrent.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.2 F7 / F4 / F11
   (feature placement, splat painting, `.sd7` build), §1.3
   (mapinfo resources block), §2.1 pitfalls 6 / 8 (splat silent-
   disable + DNTS+water LOS bug).
3. **`/home/teague/code/BARMapEditor/docs/PITFALLS.md` §15**
   (splatDetailNormalTex subtable form is preferred) **+ §16–17**
   (base normal encoding, specular exponent — for context, Sprint 9
   already addressed in the shader).
4. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`**
   §1.8 (`splatDetailNormalTex` subtable form vs legacy
   numbered keys), §6 (resource_spot_finder pipeline — for geo
   features cross-check), §10 (PyMapConv responsibility split — D6
   writes the `mapinfo.resources` block, NOT PyMapConv).
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` —
   read C6 + D6 in full. Skim D7 (Sprint 13 / minimap) since D6's
   splat pipeline produces the per-slot DDS that D7's auto-minimap
   eventually samples.
6. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/featureplacer.rs`
   (from C2 + Sprint 11 / C5 if done) — C6 grows this. C5 wrote
   `geovent` features; C6 adds the user's free-form selections.
7. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/dnts.rs`
   (from D2 / Sprint 8) — D6 calls `bake_dnts()` per active slot.
8. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/package.rs`
   — D6 adds the splat distribution PNG + per-slot DDS bundling
   into `.sd7` staging.
9. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs::resources`
   — `From<&Project> for MapInfo` populates the resources block from
   `Project.splat_config + Project.specular_tex_path` (etc).
10. `/home/teague/code/BARMapEditor/crates/barme-core/src/splat.rs`
    (from D3 / Sprint 8) — `SplatDistribution`, `SplatConfig`.
11. ADR-018 (Brush trait pattern), ADR-025/026/027 (texture pack +
    DNTS bake), ADR-029 (three-file emission), ADR-034 reserved
    (splatDetailNormalDiffuseAlpha = 1 high-pass workflow — DEFERRED).

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new stage-1-f7-features
./devlog/log.sh new stage-1-splat-pipeline
```

Fill each from phase-3-plan.md.

## Step 3 — Scope

In order, one commit per item:

### 1. C6 — F7 feature placement + stock manifest

- **Stock manifest** (`assets/mapfeatures_catalog.json`, new file
  committed):

  ```json
  {
    "version": 1,
    "source": "github.com/beyond-all-reason/mapfeatures @ <commit>",
    "categories": {
      "trees": [
        { "name": "pinetree", "display": "Pine tree", "tags": [...] },
        ...
      ],
      "rocks": [...],
      "wreckage": [...],
      "props": [...],
      "geo": [
        { "name": "geovent", "display": "Geo vent", "tags": [...] }
      ]
    }
  }
  ```

  Source: `github.com/beyond-all-reason/mapfeatures`. The session
  clones (or assumes already cloned at `/home/teague/code/mapfeatures`)
  the repo and walks its features tree. Each feature dir typically
  has a `featuredef.lua` (or similar) — the manifest captures the
  `FeatureDef` name. Categorization is heuristic (filename / tag /
  geoThermal flag).

  **For Sprint 12 scope**: ship a hand-curated 30–50 entry manifest
  covering the common stock features (pine trees, generic rocks,
  generic wreckage, the geovent). A future polish item can auto-
  generate from the `mapfeatures` repo at fetch time. Document the
  manual curation in the devlog.

- **Project model** (`crates/barme-core/src/project.rs`):
  ```rust
  pub struct FeatureInstance {
      pub name: String,           // FeatureDef name from manifest
      pub x_elmo: i32,
      pub z_elmo: i32,
      pub rot_heading: u16,       // Spring heading 0..65535
  }
  pub struct Project {
      // ...
      #[serde(default)]
      pub features: Vec<FeatureInstance>,
  }
  ```

- **Tool enum + UI** (`crates/barme-app/src/ui/inspector_feature.rs`,
  new):
  - `Tool::Feature` + `Icon::Feature` (a simple tree-line glyph).
    Keyboard `F`.
  - Inspector:
    - **CATEGORY section**: `ComboBox` to pick a category from the
      manifest (Trees / Rocks / Wreckage / Props / Geo).
    - **PICKER section**: filtered list of features in the selected
      category. Each row: small thumb (defer per-feature thumbnails
      to Stage 2 — for now render a category-icon swatch), name,
      tags. Click selects. Below the list, a search `TextEdit`
      filters by name/tag.
    - **PLACED section**: tree of placed features by name (counts
      per name). Expand a row → list of instances with X / Z /
      `rot_heading` (display as degrees: `rot * 360 / 65535`) +
      delete button.

- **Canvas interaction**:
  - LMB-click in empty space → place selected feature at cursor with
    `rot_heading = 0` (`ProjectDiff::PlaceFeature`).
  - LMB-drag on existing feature marker → rotate
    (`ProjectDiff::RotateFeature`; `rot_heading += delta * 182`,
    matching Spring's `mathAtan2 * (COBSCALE / ...)` convention used
    in `unit_sunfacing.lua:30`).
  - RMB-click on marker → delete.
  - Cross-tool ghost @ 50 % alpha when not active.

- **Symmetry**: replicate sources through `App::symmetry`. For
  rotational symmetry, the derived `rot_heading` rotates by
  `2π/fold` per copy.

- **Emission** (`crates/barme-pipeline/src/featureplacer.rs`):
  - Walk `Project.features` and emit each as:
    ```lua
    [N] = { name = "pinetree", x = 1024, z = 2048, rot = "32768" },
    ```
  - `rot` is the QUOTED-STRING heading (PITFALL §6 / Claude
    research, FINDINGS §6). Cite in commit.
  - Sprint 11 / C5's `geovent` features remain in this same file;
    coordinate ordering (sort by name then by xz for determinism).
  - Validate-at-emission: if any `feature.name` is NOT in the stock
    manifest, emit a warning (not an error — the user may have
    invoked a custom feature). Surface to the C8 linter as a
    pre-emission lint (Sprint 14).
  - Symmetry expansion happens at emission time, same pattern as F8 /
    C4 / C5.

- **Unit tests**:
  - `featureplacer::tests::renders_features_with_quoted_rot` — `rot =
    "0"` (string, not int).
  - `featureplacer::tests::geo_and_user_features_coexist` — a Project
    with both `geo_vents` and user-placed `features` emits an
    ordered single Lua table.
  - `manifest::tests::stock_geovent_present` — manifest can be
    loaded and `geovent` is findable.

### 2. D6 — splat pipeline wiring + .sd7 bundling

- **Step a — identify active slots**: at Build time, scan
  `Project.splat_config.channels`. The 4 RGBA channels each carry
  an optional `slot_id`. Active = `slot_id.is_some()` AND
  `Project.splat_distribution` has non-zero pixels in that channel.
  - Edge case: empty channels are bound to slot 0 ("grass-meadow")
    by default so the visual baseline is grass on un-painted areas.
  - Document this in ADR-027 or in `dnts.rs` comments.

- **Step b — bake DNTS per active slot**:
  - For each active channel (R/G/B/A), call:
    ```rust
    bake_dnts(
        &slot_registry.get(slot_id)?.dir,
        &staging.join(format!("{slot_name}_dnts.dds")),
        BakeOptions { diffuse_in_alpha: false }, // ADR-025 baseline
    )?;
    ```
  - Cache key from D2 ensures re-builds with identical inputs no-op.

- **Step c — splat distribution PNG**:
  - Write `Project.splat_distribution` as
    `staging/maps/<projectname>_splatdistr.png` (RGBA, dimensions
    from the distribution — 1024² fixed per Sprint 8's correction).
  - The PNG is committed at full RGBA resolution; engine reads any
    dimension (`SMFReadMap.cpp:281`).

- **Step d — `.sd7` staging**:
  - Bundle DDS files into `maps/textures/<slot_name>_dnts.dds`.
  - Bundle the splat distribution into
    `maps/<projectname>_splatdistr.png`.
  - The existing `barme-pipeline::sd7::package` machinery picks them
    up (after adding the new staging walk).

- **Step e — `mapinfo.resources` population** (PITFALL §15):
  - In `crates/barme-core/src/mapinfo_schema.rs::From<&Project> for MapInfo`,
    populate the resources block. Use the SUBTABLE form for
    `splatDetailNormalTex`:
    ```rust
    resources.splat_detail_normal_tex = SplatDetailNormalTex::Subtable {
        paths: [
            "<slot_r>_dnts.dds".to_string(),
            "<slot_g>_dnts.dds".to_string(),
            "<slot_b>_dnts.dds".to_string(),
            "<slot_a>_dnts.dds".to_string(),
        ],
        alpha: project.splat_config.diffuse_in_alpha,
    };
    resources.splat_distr_tex = Some("<projectname>_splatdistr.png".to_string());
    ```
  - Lua emit (in `mapinfo.rs`):
    ```lua
    resources = {
      splatDistrTex = "<projectname>_splatdistr.png",
      splatDetailNormalTex = {
        "<slot_r>_dnts.dds",
        "<slot_g>_dnts.dds",
        "<slot_b>_dnts.dds",
        "<slot_a>_dnts.dds",
        alpha = false,
      },
      ...
    }
    ```
  - The legacy `splatDetailNormalTex1..4` numbered keys are NOT
    emitted (PITFALL §15 — the engine prefers the subtable form;
    mixing both causes the subtable to win and the keys to be
    silently ignored, but it's noisy in the diff).

- **Step f — `splats.texScales` / `texMults` population**:
  - From `Project.splat_config.tex_scales / tex_mults`.
  - Default `0.02` / `1.0` (ADR-025 baseline).

- **Step g — specularTex graceful fallback** (PITFALL §6, §17,
  FINDINGS §7.2):
  - If `Project.specular_tex_path.is_none()`, ship a stock
    1024×1024 BC1 grey specular (~0.5, 0.5, 0.5, 0.4) into the
    staging at `maps/<projectname>_specular.dds`.
  - Emit `resources.specularTex = "<projectname>_specular.dds"`.
  - Rationale per FINDINGS §7.2: DNTS doesn't strictly require
    `specularTex` since recent Recoil, but the visual result is
    noticeably flat without it; shipping a grey default closes the
    "no spec → muddy look" lint warning.

- **Unit tests**:
  - `splat_pipeline::tests::active_slots_from_distribution` — given a
    distribution with non-zero R + G channels and bound slots, the
    active-slot set is exactly {R, G}.
  - `splat_pipeline::tests::dds_per_slot_emitted` — a build with 2
    active slots produces 2 DDS files in staging.
  - `splat_pipeline::tests::resources_subtable_form_not_legacy` —
    emitted `mapinfo.lua` contains `splatDetailNormalTex = {` AND
    does NOT contain `splatDetailNormalTex1 =`.
  - `splat_pipeline::tests::specular_fallback_when_unset` — Project
    with no `specular_tex_path` produces a grey `_specular.dds` in
    staging and `resources.specularTex` references it.

- **Integration smoke test**: build a project with the 4 default
  active slots, load in BAR, confirm the painted distribution is
  visible (matches the editor preview from Sprint 9). Record in
  devlog.

- **Touch points**:
  - `crates/barme-pipeline/src/splat_pipeline.rs` (new — wraps D2's
    `bake_dnts` + staging logic).
  - `crates/barme-pipeline/src/package.rs` (calls splat_pipeline).
  - `crates/barme-pipeline/src/mapinfo.rs::resources_block` (subtable
    emit).
  - `crates/barme-core/src/mapinfo_schema.rs::{ResourcesBlock, SplatDetailNormalTex}`
    (enum for subtable vs legacy form; default to subtable).
  - `crates/barme-core/src/project.rs::Project` (add
    `pub specular_tex_path: Option<PathBuf>`, default None).
  - Possibly `crates/barme-pipeline/src/dnts.rs` (extend cache key to
    include the slot's `meta.toml`'s `default_tex_scale` /
    `default_tex_mult` if D2 didn't already include them).

- **No new ADR** — D6 builds on the existing
  ADR-025/026/027/029/035/036 chain. The "high-pass diffuse in
  alpha" workflow (`splatDetailNormalDiffuseAlpha = 1`) remains
  ADR-034 (deferred).

### 3. Rollup commit

- STATUS UPDATEs in SRS / ROADMAP (F4 ticked — splat-emission round-
  tripped end-to-end; F7 ticked).
- Phase-3-plan.md checkboxes (C6, D6).
- closing devlog logs for both items.
- "Sprint 13 = D7 + C7 (minimap auto-generation + F9 mapinfo form
  editor)" handoff note.

## Step 4 — Standing constraints

- `source ~/.cargo/env`.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- No `Co-Authored-By: Claude`.
- Terse commit subjects.
- Local-only.
- SRS source of truth.
- Tracing: `info!` on slot bake start/end + cache hit; `warn!` on
  unknown feature name; `trace!` on per-stamp emission.
- Devlog folder per item.

## Step 5 — Out of scope

- D7 minimap auto-gen. Sprint 13.
- C7 F9 mapinfo form editor. Sprint 13.
- C8 lint pass. Sprint 14.
- Custom feature bundling (map-bundled `.s3o` + textures). Stage 2.
- `splatDetailNormalDiffuseAlpha = 1` workflow. ADR-034 deferred.
- F2 decompile import. Stage 2.
- Auto-generating `mapfeatures_catalog.json` from the
  `mapfeatures` repo. Polish task for a later sprint.

## Step 6 — Critical pitfalls (read twice)

1. **Subtable form for `splatDetailNormalTex`** (PITFALL §15, FINDINGS
   §1.8). Engine reader at `MapInfo.cpp:383-399` prefers the
   subtable; emitting numbered keys works but is silently shadowed
   if the subtable is also present. The audit's recommendation is to
   emit ONLY the subtable. Test asserts no `splatDetailNormalTex1
   =` leaks.

2. **DDS cache from D2 must include `tex_scale` / `tex_mult`** in
   its sha256 input. Otherwise a user editing `tex_mult` in the
   Inspector hits a stale cache and the bake's encoded baseline
   doesn't reflect the change. *Note*: tex_scale/tex_mult don't
   affect the DDS content (they're sampled in the fragment shader
   from `splats.tex_scales`), so this is actually NOT a cache key —
   verify whether D2 currently keys them out; correct if so.

3. **Specular fallback at 1024 BC1** is the most space-efficient
   option (~85 KB compressed). Don't ship a 4K spec; oversized
   speculars are PITFALL §6's "every bytes counts in `.sd7` size."

4. **Symmetry replication of features**: rotational fold > 2 needs
   to rotate the `rot_heading` per copy. Test this with a tank
   wreck — symmetry off shows one tank; rotational ×4 shows four
   tanks each rotated 90°.

5. **`name` validation at emission**: if a feature's `name` isn't in
   the stock manifest, the engine logs `[GetFeatureDef] could not
   find FeatureDef` and silently skips spawn. Emit a `warn!` log
   line and surface to C8 (Sprint 14) but don't gate the build —
   custom features are a Stage 2 feature.

6. **PNG dimensions for the splat distribution**: write at the
   distribution buffer's resolution (1024² per D3's fixed-dim
   correction). Don't downscale; the engine reads any size.

7. **Path conventions inside the `.sd7`**: DDS files go in
   `maps/textures/`; splat distribution PNG goes in `maps/`. The
   `mapinfo.resources` paths are relative to the archive root
   (e.g. `"maps/textures/grass-meadow_dnts.dds"`, NOT just the
   filename).

8. **No `geovent` duplication**: if Sprint 11 / C5 has shipped, the
   `geovent` features come from `Project.geo_vents` and live in
   `featureplacer.rs` already. C6 ALSO writes into the same file —
   ensure the union/de-dup logic is sane. Suggestion: emit
   `Project.geo_vents` first (sorted by xz), THEN
   `Project.features` (sorted by name then xz). One pass, two
   sections in source.

9. **Sprint 9's mapinfo gap closes here**: D5 wired the splat
   inspector to `Project.splat_config` without touching emission.
   D6's `mapinfo.resources` block is what closes the round-trip.
   Smoke test = build & load in BAR; confirm preview matches.

10. **PyMapConv does NOT touch `mapinfo.lua` / `_splatdistr.png` /
    DDS files** (FINDINGS §10). PyMapConv compiles the SMF + SMT
    only. The splat distribution PNG and per-slot DDS files are
    pure editor outputs that the editor stages directly into the
    `.sd7` (`barme-pipeline::sd7::package`). No PyMapConv flag
    invocation changes here.

## Step 7 — Exit criteria

- 3 commits on `main`: C6, D6, rollup.
- 2 devlog folders filled.
- 2 phase-3-plan.md checkboxes ticked.
- SRS / ROADMAP STATUS UPDATEs (F7 + F4 round-trip).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- `assets/mapfeatures_catalog.json` committed (30–50 entries).
- Smoke test (record in final devlog log):
  - Editor: place 3 pine trees + 2 rocks via `Tool::Feature` (`F`).
  - Place 1 geo vent (if C5 done).
  - Paint a green splat stroke (Sprint 9's UI).
  - Build & Install → load in BAR. Confirm:
    - 5 features visible in approximate positions, on the ground,
      rotated per the editor's rot values.
    - 1 geo vent visible with steam plume.
    - The painted area renders the grass slot's diffuse (matching
      the editor's preview — D4/D5 ↔ D6 round-trip).
    - No "waiting for players" hang (gates on Sprint 10).
    - No `[GetFeatureDef]` warnings in BAR's log.
  - Re-build with identical inputs → byte-identical `.sd7`
    (NFR-Determinism). Inspect via
    `sha256sum staging/*.dds && sha256sum staging/maps/*.png`.
- Final devlog log summarising what shipped + "Sprint 13 = D7 + C7
  (minimap auto-gen + F9 mapinfo form editor)" handoff note.

Start by reading the existing `crates/barme-pipeline/src/dnts.rs`
(from D2) and `crates/barme-pipeline/src/sd7.rs` — the splat-pipeline
glue you add wraps these. The featureplacer is small; the heavy
lifting is the splat-pipeline + mapinfo.resources subtable emit.
