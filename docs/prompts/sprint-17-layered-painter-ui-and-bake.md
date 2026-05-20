# Sprint 17 — Layered painter, Part 3: Layers panel UI + DNTS hybrid emission + sunset legacy splat (D10)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 17** — the last of three sprints (15 / 16 / 17) that
rebuild texture painting around a Photoshop-style layered stack.
Sprint 15 shipped the data model + CPU bake to BMP; Sprint 16
shipped the GPU composite + 2D paint viewport + tiled-COW masks.
Sprint 17 finishes the feature: a full Photoshop-style **Layers
panel**, **custom texture import** (file picker + drag-drop),
per-layer **transform / color / blend / DNTS-binding controls**, the
**DNTS hybrid emission path** (bottom ≤4 DNTS-bound layers drive the
splat distribution PNG + per-slot DDS bake), and the **retirement of
`Tool::SplatPaint` + `inspector_splat`**.

After this sprint, the user has the painter they asked for:
unlimited stylistic layers compose into the diffuse BMP at full
resolution, the bottom four DNTS-bound layers preserve real-time
per-fragment normal mapping in-game, and the legacy 4-channel splat
inspector is gone.

**Prerequisites:**
- Sprint 15 (D8) AND Sprint 16 (D9) MUST be ticked. The Layers panel
  drives the Sprint 15 data model; the DNTS hybrid bake reuses
  Sprint 16's GPU composite for the diffuse path + Sprint 15's CPU
  bake for the final `.sd7` build.
- Sprint 13 (renderer-depth rework, ADR-037) MUST be ticked. Same
  reasoning as Sprint 15/15 prereq.
- Sprint 12 (D6) MUST be ticked. The existing splat emission code
  (`crates/barme-pipeline/src/splat_pipeline.rs`) is what Sprint 17
  extends; the file must exist.

**Out of scope:**
- Undo for per-stroke mask edits (deferred since Sprint 16 — same
  reasoning here; tracked as a Sprint-20+ follow-up that grafts the
  per-stroke COW pattern from ADR-033 onto the tiled-COW mask
  storage). Mask edits remain non-undoable in Sprint 17. Layer
  add/remove/reorder/property-edits ARE undoable via the
  `ProjectDiff` variants Sprint 15 added.
- Blend modes beyond Normal — same deferral as Sprint 16.
- Pen-pressure input — egui doesn't surface tablet pressure events
  natively. Deferred.
- Live composite preview at full diffuse resolution (>4096²) — the
  Sprint 16 ADR-039 cap stays in place; the CPU bake at `.sd7`
  build time delivers the full-res output.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (mapinfo
   resources, splatDetailNormalTex subtable form — Sprint 12 wired
   the basic emission; Sprint 17 drives it from the new layer stack),
   §2.1 #6 (`splatDetailNormalTex` lint — WARN, not GATE, per
   FINDINGS §7.2).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` §15
   (subtable form), §4 (DXT1 compression).
4. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`**
   §1.8 (subtable form locked by Sprint 12), §7.3 (per-channel
   splat math — unchanged from Sprint 9; Sprint 17's contribution
   is to drive the splat distribution from the new layer masks,
   not the legacy `splat_distribution` buffer).
5. **ADRs:**
   - **ADR-038, ADR-039, ADR-040** (from Sprints 14 / 15) — the
     data model + GPU composite + paint viewport this sprint builds
     on.
   - **ADR-041 (NEW)**: Layers panel UI + DNTS hybrid emission.
     Covers the Photoshop-style panel layout, drag-to-reorder,
     custom texture import workflow, per-layer transform / color /
     blend / DNTS-binding controls, and — most load-bearing — the
     **mask → splat distribution channel** materialization that
     Sprint 17 wires into the existing splat pipeline.
   - **ADR-027** (slot registry) — custom imports add a SECOND
     source path next to the stock `tools/textures/<NN-slot>/`
     directories; ADR-041 amends ADR-027 (or supersedes a small
     section) to cover the new project-local
     `<project>/textures/<uuid>.png` entries.
   - **ADR-034** (`splatDetailNormalDiffuseAlpha = 1` workflow) —
     stays deferred per ADR-025 baseline; Sprint 17 ships the
     `diffuse_in_alpha` toggle (mirroring the legacy field) but
     wires it through the layer stack's DNTS-bound layers, not the
     deprecated `splat_config`.
6. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/splat_pipeline.rs`
   (from Sprint 12 / D6) — extends here. The mask → splat
   distribution materialization is new.
7. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/dnts.rs`
   (from Sprint 8 / D2) — `bake_dnts()` is called per
   DNTS-bound layer.
8. `/home/teague/code/BARMapEditor/crates/barme-core/src/layers.rs`
   (from Sprint 15) — extends with `dnts_layers()` accessor that
   returns the bottom ≤4 `Some(channel)` layers in channel order.
9. `/home/teague/code/BARMapEditor/crates/barme-core/src/splat.rs`
   — the legacy `SplatConfig` + `SplatDistribution`. Sprint 17
   retires both **at the project boundary** (no longer serialized
   on new project saves; a load-time migration is the one place
   they're still read). The runtime DNTS shader path from Sprint 9
   stays — it just gets its inputs from the new layer-derived
   `SplatUniforms` instead of the legacy `splat_config`.
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs::inspector_splat`
    (lines 3941-4412 at time of Sprint 9; check current line range)
    — the function this sprint deletes. Its `Tool::SplatPaint`
    enum variant + canvas pointer dispatch entry + keyboard `T`
    binding all go away. The `Icon::Splat` glyph stays (other
    surfaces might still reference it; remove only after grep
    confirms zero callers).
11. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/paint_view.rs`
    (from Sprint 16) — Sprint 17 doesn't touch the viewport itself;
    it replaces the minimal "active layer strip" with the full
    Layers panel.
12. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/widgets.rs`
    — section, chip, ramp_slider_labelled, pill_toggle. The
    Layers panel reuses these. **DO NOT** invent new widget
    primitives — extend the existing set.
13. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md`
    — D10. Confirm ADR-041 reservation.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-layers-panel-ui
./devlog/log.sh new stage-1-layers-dnts-hybrid-emission
./devlog/log.sh new stage-1-legacy-splat-retired
```

Three sub-items.

## Step 3 — Scope

Five commits on `main`:

### Commit 1 — Layers panel UI (`crates/barme-app/src/ui/layers_panel.rs`)

Photoshop-style. Replaces Sprint 16's minimal "active layer strip"
in the inspector. Specifies state changes via `ProjectDiff` so
add / remove / reorder / property-edit all flow through undo.

**Layout (top→bottom):**

- **Top header chip row**: layer count + "Add layer" split-button
  (primary action = add stock-slot layer, dropdown = "Import
  texture…" + "Duplicate active" + "Add from heightmap range").
  "Add from heightmap range" creates a layer with a mask
  procedurally generated from the heightmap (e.g., paint snow
  above 600 m) — small QOL feature, gated behind a `Coming soon`
  chip if time-boxed; ship the affordance but tooltip explains
  it's a follow-up.

- **Layer rows** (top of list = top of stack; reverse iteration of
  `Project.layers.layers`):
  - Drag handle (left) — drag to reorder. Visual: 6 dots, accent
    on hover. Emit `ProjectDiff::ReorderLayer` on drop.
  - Visibility eye toggle (was Sprint 16's bool). Click flips
    `layer.visible`; emit `ProjectDiff::SetLayerProperty`.
  - Lock toggle.
  - Layer thumbnail (~32 × 32 px). Composite of slot diffuse +
    mask preview (mask-clear pixels show a checker). Cached per
    layer id on `App::layer_thumbnails`.
  - Layer name (editable on double-click → in-place rename;
    Enter commits, Escape cancels).
  - DNTS chip: R / G / B / A / ∅ (∅ = unbound). Click cycles
    R→G→B→A→∅; right-click opens an explicit picker with the four
    channels listed + their current bindings. **Constraint**: at
    most one layer per channel; binding a channel that's already
    bound transfers the binding (the previous layer goes ∅) with
    a one-frame toast "Channel R reassigned from <layer name> to
    <this layer>" — emits TWO `ProjectDiff::SetLayerProperty`
    entries grouped into one undo step.
  - Opacity slider (0..=100 %), tiny inline. Larger control in the
    expanded layer panel below.
  - Delete button (X), right-aligned. Confirms via a small modal
    when the layer has non-empty masks (i.e. user painted on it).

- **Expanded layer panel** (the active layer's properties, collapsible
  section below the row list):
  - **Source** subsection: layer's `LayerSource`. For `Slot`,
    show the slot name + thumbnail + a "Change slot…" button
    (re-uses the Sprint 9 slot-picker grid widget). For
    `Imported`, show the file name + "Replace…" / "Open in
    file manager" / "Export mask".
  - **Transform** subsection: 6 `ramp_slider_labelled` rows:
    - Offset X (range `-elmo_extents.x..=elmo_extents.x`, suffix
      " elmos")
    - Offset Y (same)
    - Scale (range `0.1..=8.0`, log-ish via custom mapping;
      tooltip "Scale > 1 zooms IN — texture tiles more often.")
    - Rotation (range `-180..=180°`, displayed as degrees,
      stored as radians).
    - Mirror X + Mirror Y as paired `pill_toggle`s on one row.
  - **Color** subsection:
    - Tint color picker (a small RGB swatch from `egui::color_edit_button_rgb`).
    - Brightness ramp slider (range `-1.0..=1.0`).
  - **Blend** subsection: dropdown for `BlendMode` — Normal-only
    in v1 but the widget is in place for future modes; tooltip
    "Only `Normal` available; more blend modes coming."
  - **DNTS bindings** subsection — only visible when the layer
    has `dnts_channel.is_some()`:
    - `tex_scale` ramp slider (range `0.0015..=0.05`, default
      `0.02`). Same as Sprint 9's per-channel `tex_scale`.
    - `tex_mult` ramp slider (range `0.0..=4.0`, default `1.0`).
    - These map back to `Project.layers.layers[i]` — extend
      `TextureLayer` with `dnts_tex_scale: f32` and
      `dnts_tex_mult: f32` (default `0.02` / `1.0`), serde-default
      so Sprint 15/15 projects load.

- **Footer chips**:
  - Total layer count + total mask memory (computed from the COW
    tile stats). Warn chip when >256 MB.
  - "Live preview is approximate (cap 16 layers, max 4096²)" chip
    visible when the cap is hit (per Sprint 16's ADR-039).

**Pointer dispatch**: existing 2D paint viewport pointer code from
Sprint 16 hooks into the active layer (now selected via the panel).
No new pointer code in Sprint 17.

**Undo wiring**: every state change in this panel produces exactly
one `ProjectDiff` (or a grouped batch for the DNTS-channel
transfer). The undo / redo top-bar buttons step through them.

### Commit 2 — Custom texture import

- **File picker**: an "Import texture…" entry in the Add-layer
  dropdown opens `rfd::FileDialog::pick_file` (the same crate the
  project save / open flow uses). Filters: PNG, JPG, JPEG.
- **Drag-drop**: egui already surfaces dropped files via
  `ctx.input(|i| i.raw.dropped_files.clone())`. The Layers panel
  consumes them when one is dropped over the panel area; otherwise
  the central viewport's existing drop handler takes precedence.
- **Storage**: imported textures are copied into
  `<project>/textures/<uuid>.png` (project-local). The project
  file's `LayerSource::Imported { path }` carries the relative
  path so projects move-with-their-textures when the user copies
  the directory.
- **Validation**:
  - Max dimensions per side: 8192. Larger images are downsampled
    on import (with a confirmation modal). Rationale: a 16k × 16k
    texture × N layers explodes memory; 8k matches the largest BAR
    diffuse dim.
  - Min dimensions: 16. Smaller raises a warn chip ("This texture
    will tile every ~N elmos — looks like noise").
  - Format: re-encode to PNG (`image::ImageBuffer::save`) so we
    don't carry user's JPEG artifacts forward. Note in the import
    confirmation: "Re-encoded as PNG."
- **Per-import metadata**: a `meta.toml` next to the imported PNG
  carries `name`, `source_filename`, `original_dims`,
  `imported_at`. Lets future tooling surface the original name
  in the UI.

### Commit 3 — DNTS hybrid emission

Wire the bottom-≤4 DNTS-bound layers into the existing splat
pipeline from Sprint 12 / D6. The diffuse BMP continues to come
from Sprint 15's CPU bake; the splat distribution + DNTS DDS files
now derive from the layer masks instead of the legacy
`Project.splat_distribution`.

- `crates/barme-core/src/layers.rs::LayerStack`:
  - `pub fn dnts_layers(&self) -> Vec<&TextureLayer>` — returns
    the layers with `dnts_channel.is_some()`, ordered by
    `SplatChannel` (R, G, B, A). At most one per channel
    (UI-enforced from Commit 1); if the data violates this
    (corrupt project file), `dnts_layers` returns the
    bottom-of-stack winner per channel and logs a `warn!`.

- `crates/barme-pipeline/src/splat_pipeline.rs` (extend):
  - **New top-level entry: `bake_dnts_from_layers(project, slot_registry, staging) -> Result<DntsBakeOutput, Error>`**.
    Returns the list of (channel, slot_id, dds_path) tuples PLUS
    the materialized splat distribution PNG path. Called by the
    `.sd7` build path INSTEAD of the legacy
    `bake_dnts_from_splat_config` (which stays as the fallback for
    pre-Sprint-14 projects; deleted in Commit 5).
  - **Step 1 — splat distribution PNG**: allocate a fresh 1024²
    RGBA8 buffer. For each DNTS-bound layer, walk its mask:
    downsample by `mask_dim / 1024` (typical: 8192/1024 = 8× box
    filter), write into the layer's channel slot. The R+G+B+A
    invariant from Sprint 9 (`R + G + B + A ≤ 255`) is satisfied
    by construction because at most one channel is bound per
    layer and the channels are independent.
    - Edge case: a fully-empty mask (the layer is hidden but
      still bound to a channel) produces an all-zero channel — the
      engine's DNTS sampler treats that as "no contribution",
      visually identical to unbinding. Document this.
  - **Step 2 — per-slot DDS bake**: call `bake_dnts(...)` for each
    bound layer's slot. Output paths follow Sprint 12's convention
    (`staging/maps/textures/<slot_name>_dnts.dds`). For imported-
    texture layers, **the DNTS slot binding is ignored** (we don't
    have a stock normal map for an imported diffuse; ADR-034's
    `diffuse_in_alpha` workflow is the only path that handles
    imported textures, and it's deferred). Imported-but-DNTS-bound
    layers raise a lint warning (one-shot, surfaced in the
    Validation chip).
  - **Step 3 — `mapinfo.resources` emit**: identical to Sprint 12 /
    D6's subtable form. The only difference is the slot list now
    comes from `dnts_layers()` instead of
    `Project.splat_config.channels`.
  - **Step 4 — `splats.texScales` / `texMults`**: read from
    `TextureLayer.dnts_tex_scale` / `dnts_tex_mult`. Same default
    `0.02` / `1.0`.

- `crates/barme-pipeline/src/lib.rs::build_sd7`:
  - Replace the call to the legacy `bake_dnts_from_splat_config`
    with `bake_dnts_from_layers`. The fallback path for
    pre-Sprint-14 projects (no layers, legacy splat_config) goes
    through a `if project.layers.layers.is_empty() { legacy }`
    branch — kept for ONE sprint; Commit 5 deletes it.

- Tests:
  - `splat_pipeline::tests::layers_dnts_emit_matches_sprint_12_format`
    — a project with 2 DNTS-bound layers (R + B) → emitted
    mapinfo.lua `splatDetailNormalTex = { ... }` matches Sprint 12's
    subtable form byte-for-byte (modulo the slot names).
  - `splat_pipeline::tests::imported_layer_dnts_lint` — a project
    with a DNTS-bound imported-texture layer → emit succeeds but
    surfaces a `LintWarning::ImportedLayerDnts` in
    `BuildOutput.warnings`.
  - `splat_pipeline::tests::mask_to_splat_distr_invariant_holds` —
    randomized: 4 DNTS-bound layers, each with random mask values,
    materialized distribution has every pixel satisfying
    `R + G + B + A ≤ 255`.

### Commit 4 — Retire `Tool::SplatPaint` + `inspector_splat`

- `crates/barme-app/src/main.rs`:
  - Delete `Tool::SplatPaint` variant + all its match arms.
  - Delete `inspector_splat` (function + all its sub-helpers
    `slot_thumbnail`, `bind_slot_to_channel`, `unbind_channel`,
    `splat_picker_open_for`, `splat_brush_state` etc.).
  - Delete `SplatBrushState` struct.
  - **DON'T DELETE** `SplatBrushRegistry` — it's still used by the
    Sprint-9 splat shader path for runtime DNTS. The brush dispatch
    is dead code at this point; remove only the App-side state.
  - **DON'T DELETE** `App::splat_config` / `splat_distribution`
    state YET — Commit 5 handles them.

- Migration toast: when a user loads a pre-Sprint-14 project, the
  `after_load_migrate` from Sprint 15 already seeded layers from
  `splat_config`. Sprint 17 adds a one-frame info toast: "Your
  project's splat layers were migrated to the new Layers panel.
  The old painting was discarded; re-paint into the layer masks.
  Sorry for the inconvenience — this is a one-time migration."
  Dismissable, persisted per-project via a new
  `Project.migration_toast_dismissed: bool`.

### Commit 5 — Retire legacy `splat_config` + `splat_distribution`

- `crates/barme-core/src/project.rs`:
  - Mark `splat_config` and `splat_distribution` as
    `#[serde(skip_serializing)]`. New projects no longer save
    them; old projects still load them, run the
    `after_load_migrate` once, and on next save the legacy fields
    drop. (Sprint 15's load-migrate is idempotent — empty layers
    stack → seed from splat_config. After seeding, the layers
    stack is non-empty; subsequent loads skip migration.)
  - Add a `LoadDeprecation::SplatConfig` enum variant logged when
    a legacy project loads. Surfaced as an `info!` log + a
    one-time toast (see Commit 4).

- `crates/barme-pipeline/src/splat_pipeline.rs`:
  - Delete `bake_dnts_from_splat_config` (the fallback added in
    Commit 3). The `build_sd7` branch on
    `if project.layers.layers.is_empty()` becomes "panic with a
    clear migration error" — but that path is unreachable for
    projects loaded through the editor (the migration runs at
    load time). Test harnesses building a bare `Project` should
    populate `Project.layers` directly.

- Tests:
  - `project::tests::legacy_splat_config_one_shot_migration` —
    save a pre-Sprint-14 project; load; assert
    `Project.layers.layers.len() == 4` and the legacy
    `splat_config.channels` match the bound layers. Save the
    migrated project; reload; assert the legacy fields are
    NOT in the on-disk file (`!file_contents.contains("splat_config")`).

### Commit 6 — Rollup

STATUS UPDATEs in SRS / ROADMAP, tick D10 in `phase-3-plan.md`,
write ADR-041 in `docs/DECISIONS.md`, amend ADR-027 with the
project-local imported-texture path entry, close the three devlog
folders. The rollup commit's body should call out:
- Layers panel UI complete.
- Custom textures imported with offset / scale / rotation / tile
  in both directions.
- DNTS hybrid emission shipped — bottom 4 DNTS-bound layers drive
  runtime per-fragment normal mapping in BAR.
- Legacy `inspector_splat` + `Tool::SplatPaint` + `splat_config` +
  `splat_distribution` retired.
- The user's original "the textures of the end map are quite
  incredibly ugly" report is closed; record the visual
  before/after in the devlog (screenshots welcome).

## Step 4 — Standing constraints

Same as Sprints 14 / 15. `cargo fmt && cargo clippy --workspace
--all-targets -- -D warnings && cargo test --workspace` green
before every commit.

Tracing: `info!` on layer add / remove / DNTS-channel rebinding /
import-texture; `debug!` on per-mask-tile bake; `warn!` on
imported-layer DNTS lint, oversized imported texture, channel
collision auto-resolve.

## Step 5 — Out of scope (loud)

- Pen pressure (egui limitation).
- Multi-layer selection / multi-edit (the panel selects ONE active
  layer at a time).
- Layer groups / folders (Photoshop has them; deferred —
  most BAR maps top out at ~6 layers, groups are overkill).
- Adjustment layers / curves / non-destructive filters (way out of
  scope).
- Stamps / pattern brushes (out of scope for v1; the slot diffuse
  IS the pattern).
- Texture import from URL / web (file picker only).
- DDS / TGA / TIFF import (PNG + JPG only). Users with DDS sources
  can convert externally.

## Step 6 — Critical pitfalls (read twice)

1. **At most one layer per DNTS channel.** The UI enforces this
   on rebind (Commit 1's channel transfer logic). The data model
   does NOT enforce it (the schema allows multiple). The
   build path's `dnts_layers()` picks the bottom-most winner per
   channel and logs `warn!`. If you ever add a CLI / scripting
   path that bypasses the UI, this invariant breaks silently —
   pin it with a `validate_layers` lint in Sprint 17 / C8.
2. **The downsample from mask resolution to 1024² (splat distribution)
   must be a box filter, not nearest-neighbour.** Nearest produces
   visible blockiness in the per-fragment DNTS blend at runtime.
   Test the smoothness in the lint pass.
3. **`R + G + B + A ≤ 255` is preserved by construction** — at most
   one channel bound per layer means the channels never collide.
   But if the user re-painted a Sprint-15-era project with multiple
   bindings to the same channel before Sprint 17's enforcement
   kicked in, the build path must STILL produce a valid splat
   distribution. The downsample step floors-down on ties (winner
   = bottom-most layer in stack order); document.
4. **Imported textures must NOT live in the project root.** They
   live in `<project>/textures/<uuid>.png` (a sibling directory
   to `<project>.barmeproj`). `LayerSource::Imported { path }`
   stores a path RELATIVE to the project root; absolute paths get
   normalized at save and rejected at load with a one-shot lint
   warning.
5. **Saving with imported textures must update the project on
   disk.** A user who imports a texture and quits without saving
   loses the texture (or gets a dangling `Imported` entry).
   Mark dirty IMMEDIATELY on import; auto-save the texture file
   regardless of the project file's save state.
6. **`splat_distribution.png` lives in `staging/maps/` per Sprint
   12, NOT `staging/maps/textures/`.** Sprint 17's hybrid bake
   writes to the same paths Sprint 12 wired. Don't accidentally
   relocate.
7. **The legacy `Tool::SplatPaint` keyboard binding (`T`) must
   not collide with the new Layers panel** — keyboard `T` is now
   free; do NOT repurpose it in this sprint (let the user pick a
   binding in a future keybinding pass).
8. **`Project.splat_config.tex_scales` / `tex_mults` migrate into
   per-layer `dnts_tex_scale` / `dnts_tex_mult`.** Sprint 15's
   `migrate_from_splat_config` did NOT copy these values
   (intentionally — Sprint 15 was data-only). Sprint 17's
   migration extension MUST: when copying a channel's binding to
   a layer, also copy the channel's scale + mult.
9. **`diffuse_in_alpha` is a per-DNTS-emit flag, not per-layer.**
   The flag corresponds to BAR's
   `splatDetailNormalDiffuseAlpha` field (ADR-034, deferred). For
   Sprint 17, expose ONE global toggle in the Layers panel footer
   that mirrors the legacy `splat_config.diffuse_in_alpha`. New
   field on `Project` (NOT on `LayerStack`):
   `Project.dnts_diffuse_in_alpha: bool` (default false, serde-
   default).
10. **The legacy `splat_config` retention for ONE more sprint**:
    Sprint 17 / Commit 5 removes serialization. If a critical bug
    surfaces post-Sprint-16, rollback is possible because the
    legacy code paths are deleted, not the schema. A future sprint
    can fully drop the struct.
11. **Drag-drop has TWO targets.** The central viewport (already
    has a drop handler from earlier sprints for heightmap PNGs)
    AND the new Layers panel. Disambiguate by drop position:
    over the Layers panel → texture import; over the viewport →
    existing heightmap handler. Document the priority in the
    Layers panel hover state ("Drop here to add as layer").
12. **Mask-to-splat downsample memory.** 8192² mask × 4 layers =
    256 MB of byte data being downsampled. Use a streaming box-
    filter that processes one 256-tile row at a time — don't
    allocate the whole 8192² float buffer at once.
13. **Custom texture import is a SCHEMA change.** Sprint 15
    shipped `LayerSource::Imported` but no consumer. Sprint 17
    wires it. Any future schema bump must respect the existing
    `Imported { path: PathBuf }` representation.

## Step 7 — Exit criteria

- 6 commits on `main`: Layers panel UI, custom texture import,
  DNTS hybrid emission, retire `Tool::SplatPaint` + inspector,
  retire legacy `splat_config`, rollup.
- 3 devlog folders filled.
- 1 checkbox ticked in `phase-3-plan.md` (D10).
- ADR-041 in `docs/DECISIONS.md`. ADR-027 amended.
- SRS / ROADMAP STATUS UPDATEs (the F4 feature row finally goes
  fully green — diffuse composition is in the editor's hands, the
  hybrid DNTS path drives runtime detail, the user's original
  pain point is closed).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
  && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Launch the editor on a fresh project → Layers panel shows
    ONE base layer auto-populated from the wizard's biome.
  - Click "Add layer" → a new layer appears at the top of the
    stack with an empty mask. Switch to `Tool::PaintLayer` (`L`),
    paint with `mask-reveal` over half the map → the new layer's
    texture shows on the painted half, the base biome shows on
    the rest. 3D viewport reflects the same composite.
  - Drag the new layer below the base → composite updates
    instantly (Sprint 16's dirty-rect upload). Drag it back to
    the top.
  - Click "Import texture…" → file picker opens; pick a 2048²
    PNG of (e.g.) cracked clay; confirm dimensions are accepted
    without downsample. New layer appears with the imported
    texture as source. Paint into its mask → it's visible.
  - Open the imported layer's properties → drag the Offset X
    slider → texture pans under the mask. Drag Scale → texture
    zooms; tiling stays seamless in both directions. Rotate to
    37° → texture rotates under the mask without stretching.
  - Bind the imported layer to DNTS channel R → warn chip appears
    ("Imported layers don't contribute to runtime normal
    detail"). Unbind.
  - Bind a slot-sourced layer to DNTS channel R. Set its
    `tex_scale` to `0.005`.
  - Build a `.sd7` → confirm:
    - `<project>_splatdistr.png` exists in the staging,
      contains the new layer's mask downsampled to 1024² in the
      R channel.
    - `grass-meadow_dnts.dds` (or whichever slot you bound)
      exists.
    - `mapinfo.lua` `splats.texScales = {0.005, 0.02, 0.02, 0.02}`.
    - `mapinfo.lua` `resources.splatDetailNormalTex = { "...dds", ..., alpha = false }`.
  - Load in BAR → painted layers visible at full diffuse
    resolution; the DNTS-bound layer's normal map kicks in at
    close range (you see the BAR per-fragment lighting respond
    to camera angle changes).
  - Open a pre-Sprint-14 project → one-time migration toast
    appears; layers stack contains 4 layers; save → reload →
    legacy `splat_config` no longer in the `.barmeproj` file.
  - `cargo test --workspace -- layers splat_pipeline` all green.
- Final devlog log: closes the user's 2026-05-19 painter-quality
  thread; references the SRS STATUS UPDATEs.

Start by running `git status`, then re-reading Sprint 9's
`inspector_splat` so you know exactly what's being deleted in
Commit 4. Begin with Commit 1 (Layers panel UI) — the layout
controls everything downstream. Commit 2 (import) plugs into the
panel, Commit 3 (DNTS emission) is pure pipeline work, Commits 4-5
are deletions that depend on 1-3 being merged so the user has the
new UI before the old UI disappears.
