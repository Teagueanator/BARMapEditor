# Sprint 15 — Layered painter, Part 1: data model + bake-to-diffuse (D8)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 15** — the first of three sprints (15 / 16 / 17) that
rebuild texture painting around a Photoshop-style layered stack. After
this sprint, the `.sd7` exported from the editor stops shipping a
synthetic biome-ramp BMP and starts shipping a real composited diffuse
from a layer stack. There is **no painting UI yet** — Sprint 16 lands
the paint viewport, Sprint 17 lands the Layers panel + DNTS hybrid
emission. Sprint 15 is data model + export bake only.

**Why this exists.** The user reported on 2026-05-19 that the diffuse
textures of exported maps are "incredibly ugly." Root cause: the
.sd7 export currently bakes a synthetic biome ramp from the heightmap
(`crates/barme-app/src/launcher.rs::synth_biome_bmp`, added in
commit `f1ab09b`) and ships that. Real BAR maps composite multiple
hand-painted texture layers at the full 512 × SMU diffuse resolution
(e.g. TitanDuel at 8192² for 16 SMU). The four-channel RGBA splat
distribution we paint into today is BAR's runtime DNTS detail
mechanism, not its primary diffuse channel — it adds per-fragment
normal/spec detail on top of the SMT-tile diffuse, it does NOT
replace it.

**The new model.** A `LayerStack` lives on `Project.layers`. Each
`TextureLayer` carries a source (slot id or imported path), a 2D
transform (offset, scale, rotation, mirror), a tint/brightness,
a blend mode (normal only for v1), an optional DNTS channel binding
(`Option<SplatChannel>`), and a per-layer alpha mask sized to the
diffuse (`512 × SMU` per side). The stack composites back-to-front
into the diffuse BMP at export time; the bottom ≤4 DNTS-bound
layers also drive splat distribution + DNTS DDS bake (Sprint 17).
This sprint ships the data structures, the CPU bake routine, and the
.sd7 hookup. **No GPU work, no UI work.**

**Prerequisites:**
- **Sprint 13 (renderer-depth rework, ADR-037) MUST be ticked.**
  All visual judgment during painter development depends on the
  renderer faithfully drawing what the bake produces. With Sprint 13
  in place, ugly-looking output during this sprint can be attributed
  to the bake / source textures, not to a renderer bug. Painter work
  CANNOT start before Sprint 13 closes.
- Sprint 12 (C6 + D6) MUST be ticked — D6 ships the `.sd7` splat
  pipeline + `mapinfo.resources.splatDetailNormalTex` subtable form
  that Sprint 17 hooks into. The bottom-4 DNTS emission Sprint 17
  drives is a refinement of that path, not a replacement, so D6 must
  land first.
- Sprints 1–12 all done.

**Out of scope:**
- Painting masks (Sprint 16).
- The Layers panel UI in the inspector (Sprint 17).
- The DNTS hybrid emission (Sprint 17) — Sprint 15 emits the diffuse
  BMP only. Splat distribution + DNTS DDS continue to come from
  Sprint 12's D6 path unchanged.
- Sunsetting the existing `inspector_splat` (Sprint 17) — Sprint 15
  leaves it in place; the user sees no UI change.
- Undo for per-stroke mask edits — there are no strokes yet. Layer
  add/remove/reorder/property edits DO go through `ProjectDiff`.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules. The
   "single-binary, single-source-of-truth" principle is load-bearing
   for the migration path below.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.2 (texture pipeline:
   `512 × SMU` diffuse, 32×32 DXT1 tiles, mandated `compressionType=1`),
   §1.3 (mapinfo resources block — DNTS layers operate ON TOP OF the
   diffuse, not in place of it), §2.1 #1 (texture pipeline memory),
   §2.1 #2 (DXT1 compression cost — the user's composited diffuse
   gets lossy-compressed by PyMapConv; expect some chroma drift).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §6
   (`splatDetailNormalTex` requires `specularTex` is now WARN-not-GATE
   per FINDINGS §7.2; this sprint inherits Sprint 9's lint), §4
   (DXT1 lossy compression).
4. `/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`
   §1.8 (subtable form for `splatDetailNormalTex`, locked by D6),
   §7.3 (per-channel splat math, locked by Sprint 9).
5. **The two ADRs you'll write or update:**
   - **ADR-038 (NEW)**: Layered texture stack data model. Owns the
     `LayerStack` / `TextureLayer` schema, migration rules from
     `Project.splat_config`, the back-to-front normal-blend
     compositor, the `512 × SMU` mask sizing rule, and the export
     contract (Sprint 15 ships ONE consumer — `synth_biome_bmp`
     replacement; Sprints 14/15 add more).
   - **ADR-018** (Brush trait — already shipped): the layer mask
     brushes in Sprint 16 use this pattern. Sprint 15 doesn't add
     brushes, but the masks must be in a shape that ADR-018-style
     dirty-rect uploads CAN target — sketch a `LayerMask::write_rect`
     stub even though Sprint 15 doesn't call it.
6. `/home/teague/code/BARMapEditor/crates/barme-core/src/splat.rs` —
   the current 4-channel splat data model. **You do NOT delete this
   in Sprint 15.** Migration happens in Sprint 17. Sprint 15 reads
   `splat_config.channels[i]` on project load and SEEDS a `LayerStack`
   with one layer per bound DNTS slot, but `splat_config` itself
   stays as the source-of-truth for runtime splats until Sprint 17.
7. `/home/teague/code/BARMapEditor/crates/barme-core/src/project.rs` —
   `Project`, `ProjectDiff`. Note `splat_config` is already on the
   struct (line 91); you add `layers` next to it.
8. `/home/teague/code/BARMapEditor/crates/barme-core/src/undo.rs` —
   undo + `ProjectDiff` patterns. Layer-level ops go through
   `ProjectDiff::AddLayer / RemoveLayer / ReorderLayer / SetLayerProperty`.
9. `/home/teague/code/BARMapEditor/crates/barme-core/src/map_size.rs` —
   `MapSize::texture_dims() = 512 × SMU`. This is the mask size per
   layer. For a 16×16 map that's 8192² × 1 byte = 64 MB per layer
   resident. **Tiled COW (ADR-018 pattern adapted for grayscale)
   is in-scope for Sprint 16 — Sprint 15 ships flat `Vec<u8>` masks
   with a `// TODO(tiled-cow)` flag at the alloc site.**
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/launcher.rs:253-300`
    — the `synth_biome_bmp` function. Sprint 15 calls
    `LayerStack::bake_diffuse` in its place when the project has a
    non-empty stack; falls back to the old biome ramp when the
    stack is empty (single-layer migration default ensures the
    stack is never empty for new projects, but old projects that
    serialize WITHOUT a layers block must still bake — see migration
    section below).
11. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md`
    — read your **D8** entry (added by Sprint 12 → Sprint 15 prep,
    or added in this sprint's first commit if not present). Confirm
    item ID + ADR reservation. **If D8 is not yet listed, add it
    under Stream D between D7 and D8's predecessor in your first
    commit.** Reserve ADR-038 in the ledger.
12. `/home/teague/code/BARMapEditor/docs/research/textures/claude-findings-from-research.md`
    and `/home/teague/code/BARMapEditor/docs/research/splat-rendering/`
    if present — Stream D's research digests. The hybrid bake model
    (Sprint 17) builds on §7.3 of FINDINGS; Sprint 15 only needs the
    BMP dimension contract from SRS §1.2.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-layers-data-model
```

Fill from D8 in `phase-3-plan.md`. One devlog folder for Sprint 15
since it's a single Stream-D item.

## Step 3 — Scope

Three commits on `main`:

### Commit 1 — Data model: `crates/barme-core/src/layers.rs` (new file)

Add the layered-stack data model. Pure-data + a single CPU
compositor; no GPU code, no UI code.

```rust
//! Layered texture stack (D8 / Sprint 15, ADR-038).
//!
//! Sits above [`crate::splat`] (the BAR-runtime 4-channel splat
//! distribution) and below the .sd7 export bake. A [`LayerStack`]
//! holds N [`TextureLayer`]s, each carrying a source (slot id or
//! imported path), a 2D transform, blend params, an optional
//! [`SplatChannel`] binding, and a per-layer alpha mask sized to the
//! map's diffuse dims (`512 × SMU` per side).
//!
//! Sprint 15 ships:
//! - The data model + serde.
//! - Migration from `Project.splat_config` (one layer seeded per
//!   bound DNTS slot at load time).
//! - The CPU compositor [`LayerStack::bake_diffuse`].
//! - `Project::layers` field + `ProjectDiff` variants for
//!   add / remove / reorder / set-property.
//! - Replacement of `synth_biome_bmp` in `launcher.rs` with a
//!   `LayerStack::bake_diffuse` call (with fallback to the biome
//!   ramp when the stack is empty — covers pre-D8 projects loaded
//!   without a layers block).
//!
//! Sprint 16 adds: tiled COW masks, layer mask brushes, the GPU
//! composite preview shader, the top-down 2D paint viewport.
//!
//! Sprint 17 adds: Photoshop-style Layers panel, custom texture
//! import (file picker + drag-drop), DNTS hybrid emission
//! (bottom ≤4 DNTS-bound layers drive splat distribution + DDS
//! bake), retirement of `inspector_splat`.

use crate::{MapSize, splat::SplatChannel};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
```

Schema:

```rust
/// One layer in the stack. Sources are resolved at bake time:
/// `Slot` indexes into the `tools/textures/<NN-slot>/` registry,
/// `Imported` is a project-local path under `<project>/textures/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayerSource {
    /// Stock slot id (ADR-027 registry).
    Slot { id: u8 },
    /// User-imported texture, project-local (Sprint 17 wires the
    /// import workflow; Sprint 15 supports the schema only).
    Imported { path: PathBuf },
}

/// Per-layer affine transform. Sampling is wallpaper-tiled in both
/// directions; the transform places the texture under the mask.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerTransform {
    /// Offset from map center in elmos.
    pub offset_elmos: [f32; 2],
    /// Uniform scale. 1.0 = native texture size.
    pub scale: f32,
    /// Rotation in radians (any angle).
    pub rotation_rad: f32,
    /// Mirror flags.
    pub mirror_x: bool,
    pub mirror_y: bool,
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            offset_elmos: [0.0, 0.0],
            scale: 1.0,
            rotation_rad: 0.0,
            mirror_x: false,
            mirror_y: false,
        }
    }
}

/// Per-layer color modulation, applied to the sampled diffuse before
/// the mask + blend.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerColor {
    /// RGB tint multiplier (1.0 = identity).
    pub tint_rgb: [f32; 3],
    /// Brightness add (-1.0..=1.0; 0.0 = identity).
    pub brightness: f32,
}

impl Default for LayerColor {
    fn default() -> Self {
        Self {
            tint_rgb: [1.0, 1.0, 1.0],
            brightness: 0.0,
        }
    }
}

/// v1: normal (alpha-over) only. Enum reserved for Sprint-N
/// expansion; Sprint 15's compositor matches on this and panics
/// on unknown variants under `debug_assertions` so a future
/// `Multiply` addition can't silently degrade to `Normal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlendMode {
    Normal,
}

impl Default for BlendMode {
    fn default() -> Self { Self::Normal }
}

/// Grayscale alpha mask, sized to the diffuse (`512 × SMU` per side).
/// Storage: flat `Vec<u8>`. Sprint 16 will swap this for a tiled-COW
/// structure but the public API (write_rect, sample_bilinear) stays.
///
/// `value[i] = 255` = layer fully visible at pixel i.
/// `value[i] = 0`   = layer fully transparent at pixel i (lower
///                    layers show through).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerMask {
    pub width: u32,
    pub height: u32,
    /// Default initial fill (255 = fully visible). The base layer's
    /// mask is full at construction; subsequent layers should pick
    /// 0 (Sprint 17 UI default) so adding a layer doesn't blow away
    /// the layers below.
    #[serde(with = "mask_bytes_b64")]
    pub bytes: Vec<u8>,
}

mod mask_bytes_b64 {
    // 64 MB raw → ~85 MB base64 is fine for the .barmeproj file but
    // base64-encode for TOML safety. Sprint 16 swaps for sidecar PNG
    // (mask becomes a project-local `<project>/masks/<layer_id>.png`)
    // when the tiled-COW model lands; the schema migration is
    // tracked under D9.
    // ...impl elided in the spec; full impl in commit.
}

/// One layer. `id` is stable across sessions so undo + sidecar files
/// (Sprint 16+) can target the right layer after reorders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureLayer {
    /// Stable identifier (UUID v4 string). Persisted; used by undo
    /// and by Sprint-14 sidecar mask files.
    pub id: String,
    /// User-visible name. Defaults to source name on creation.
    pub name: String,
    pub source: LayerSource,
    pub transform: LayerTransform,
    pub color: LayerColor,
    pub blend: BlendMode,
    /// Layer is included in the bake. Eye-toggle in the Layers panel
    /// (Sprint 17) flips this.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Layer is locked from edits in the Layers panel (Sprint 17).
    #[serde(default)]
    pub locked: bool,
    /// `Some(channel)` = this layer is one of the (up to 4) DNTS-bound
    /// layers; Sprint 17's emitter wires its mask into the splat
    /// distribution's matching channel + bakes the slot's normal map
    /// into the DDS array. `None` = layer is diffuse-only.
    #[serde(default)]
    pub dnts_channel: Option<SplatChannel>,
    /// Per-layer opacity multiplier on top of the mask. 1.0 = identity.
    /// 0.0 ≡ visible=false but keeps the mask data live.
    #[serde(default = "default_one_f32")]
    pub opacity: f32,
    pub mask: LayerMask,
}

fn default_true() -> bool { true }
fn default_one_f32() -> f32 { 1.0 }

/// The layer stack. Z-order is [`Vec::iter`] from bottom (idx 0)
/// to top (idx N-1). The Layers panel UI in Sprint 17 will render
/// reversed (Photoshop convention: top of list = top of stack); the
/// internal order stays "bottom-first" so the compositor iterates
/// naturally.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerStack {
    pub layers: Vec<TextureLayer>,
}
```

API:

```rust
impl LayerStack {
    /// Build the default stack for a fresh project from the biome
    /// choice in the wizard. Always exactly ONE layer: the biome's
    /// "base" slot from `tools/textures/`. Mask = full visible.
    pub fn from_biome(biome: &str, size: MapSize) -> Self;

    /// Migrate from a pre-D8 `Project.splat_config` to a layer
    /// stack. One layer per bound DNTS channel, in R/G/B/A order,
    /// each with `dnts_channel = Some(channel)`. Masks start FULL
    /// (255) for the bottom layer and EMPTY (0) for the rest — the
    /// pre-D8 splat painting is NOT migrated to mask pixels in
    /// Sprint 15 (the data shape is different; Sprint 17 will offer
    /// a one-time migration when the user opens an older project).
    /// The pre-D8 `splat_config.tex_scales` / `tex_mults` are
    /// preserved on `Project.splat_config` for runtime DNTS — they
    /// are NOT migrated into the layer model in Sprint 15.
    pub fn migrate_from_splat_config(
        config: &crate::SplatConfig,
        slot_id_for_channel: impl Fn(u8) -> Option<u8>,
        size: MapSize,
    ) -> Self;

    /// Bake all visible layers into an RGB8 diffuse image. Output
    /// dims = `size.texture_dims()` (`512 × SMU` per side). This is
    /// the BMP fed to PyMapConv at `.sd7` build time, replacing
    /// `synth_biome_bmp`.
    ///
    /// Compositor: back-to-front (idx 0 → N-1). For each pixel,
    /// sample the layer's source diffuse with wallpaper-tiled
    /// modulo + transform, apply tint/brightness, multiply by
    /// `(mask × opacity)`, alpha-over the current accumulator.
    ///
    /// Performance: 8192² × 16-layer = ~256 megasamples worst case.
    /// Rayon-parallelize per-row. Target: ≤1.5 s release for a
    /// 16-SMU map with 8 layers. Profile + record in the devlog.
    pub fn bake_diffuse(
        &self,
        size: MapSize,
        slot_resolver: &impl SlotResolver,
    ) -> image::RgbImage;
}

/// Trait so the bake doesn't reach into UI / app crates.
pub trait SlotResolver {
    /// Returns the resolved diffuse path for a slot id, or None if
    /// missing. Caller wraps a registry walk.
    fn diffuse_path(&self, slot_id: u8) -> Option<std::path::PathBuf>;
}
```

Migration story for pre-D8 projects (covered by tests):
- Load a `.barmeproj` saved before Sprint 15 → `Project.layers`
  serde-deserializes as `LayerStack::default()` (empty).
- A new `Project::after_load_migrate(&mut self)` runs once and, if
  `self.layers.layers.is_empty()`, calls
  `LayerStack::migrate_from_splat_config(...)`. This keeps the user's
  slot bindings; their painted splat distribution is preserved on
  `splat_config` for the runtime DNTS path until Sprint 17 retires it.
- Test: pre-D8 fixture → post-load stack has 4 layers, slot bindings
  match the channel bindings, masks have correct initial state.

### Commit 2 — `Project::layers` + `ProjectDiff` + load-time migration

- `crates/barme-core/src/project.rs`:
  - Add `pub layers: LayerStack` next to `splat_config` (line 91).
    `#[serde(default)]` so old project files load.
  - Implement `Project::after_load_migrate(&mut self, slot_resolver: &impl SlotResolver)`
    that seeds layers from `splat_config` on first open.
  - `Project::new` seeds a single-layer stack from the wizard's
    biome (`LayerStack::from_biome`).
- `crates/barme-core/src/undo.rs`:
  - Add four `ProjectDiff` variants:
    ```rust
    AddLayer { index: usize, layer: TextureLayer },
    RemoveLayer { index: usize, layer: TextureLayer },
    ReorderLayer { from: usize, to: usize },
    SetLayerProperty {
        layer_id: String,
        from: LayerPropertyValue,
        to:   LayerPropertyValue,
    },
    ```
    `LayerPropertyValue` is a small enum covering transform / color /
    blend / visible / locked / opacity / dnts_channel / name. Mask
    edits are NOT a ProjectDiff (deferred to Sprint 16; Sprint 15
    cannot edit masks).
  - `apply` / `revert` impls; round-trip tests.

### Commit 3 — Hook the bake into the export path

- `crates/barme-app/src/launcher.rs::build_and_install`:
  - Replace the call to `synth_biome_bmp` with:
    ```rust
    if project.layers.layers.is_empty() {
        synth_biome_bmp(...)  // kept as fallback for projects whose
                              // load-time migration was skipped
                              // (e.g., test harnesses building a
                              // bare Project directly)
    } else {
        let img = project.layers.bake_diffuse(project.size, &registry);
        write_rgb_as_bmp(&img, &tex_bmp_path)?;
    }
    ```
  - `write_rgb_as_bmp` exists nowhere yet — add it in
    `crates/barme-pipeline/src/sd7.rs` (or wherever the existing BMP
    writer lives). 24-bit RGB BMP, dims multiple of 1024 per side
    (`MapSize::texture_dims` already guarantees this — `512 × SMU`
    is always a multiple of 512, and `SMU ≥ 2` makes it a multiple
    of 1024).
- `synth_biome_bmp` stays as the empty-stack fallback; the tests in
  `launcher.rs::biome_ramp_thresholds_match_wgsl` continue passing.
- Integration test: build a project with a single base layer →
  resulting BMP is the slot's diffuse, wallpaper-tiled to fill the
  texture dims. Dimensions exact (`MapSize::texture_dims`); no
  truncation. Loaded round-trip through the BMP reader matches the
  bake output within a 2/255 tolerance (BMP byte-quantization).
- Smoke test: `cargo run -p barme-pipeline --example build_smoke`
  (already exists) produces a `.sd7` whose `.smt` slice no longer
  shows the biome ramp. Add a NEW example
  `crates/barme-app/examples/bake_layered_smoke.rs` that builds a
  project with TWO layers (base + a half-covered second layer at
  50 % opacity), runs the bake, and writes the BMP to
  `/tmp/bake_layered_smoke.bmp` for visual inspection. Document the
  command line in the devlog.

Then a **4th rollup commit**: STATUS UPDATEs in SRS / ROADMAP, tick
D8 in `phase-3-plan.md`, ADR-038 in `docs/DECISIONS.md`, close the
devlog log with a "Sprint 16 = paint viewport + GPU composite" handoff.

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells.
- Before every commit: `cargo fmt && cargo clippy --workspace
  --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects (`layers: ...`, `core: ...`, `pipeline: ...`).
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing: `info!` on bake start + duration; `debug!` on per-layer
  sample stats; `warn!` on missing slot diffuse (bake skips that
  layer with a placeholder grey fill, not a panic).
- Devlog folder per item.

## Step 5 — Out of scope (loud)

- **No painting UI in this sprint.** `inspector_splat` stays
  exactly as Sprint 9 left it. Users cannot create / delete / reorder
  layers from the editor yet — that's Sprint 17. The migration
  produces a layer stack but the user can't see it.
- **No GPU composite pipeline.** The CPU bake is the only consumer
  of the stack in Sprint 15. The 3D viewport's diffuse continues to
  come from Sprint 9's WGSL composite (biome ramp + slot diffuses).
- **No tiled-COW masks.** Flat `Vec<u8>` per mask. Memory for big
  maps with many layers is monitored but unbounded; the `// TODO`
  comment at the alloc site flags the deferral. Sprint 16 lands the
  COW upgrade.
- **No custom texture import.** `LayerSource::Imported` is in the
  schema but unreachable from any UI in Sprint 15. Sprint 17 wires
  the file picker.
- **No DNTS hybrid emission.** Sprint 15's bake produces the diffuse
  BMP only. Splat distribution + DNTS DDS continue to come from
  Sprint 12 / D6 untouched. Sprint 17 wires the hybrid path.

## Step 6 — Critical pitfalls (read twice)

1. **`Project.layers` must `#[serde(default)]`.** Without it, every
   pre-Sprint-13 `.barmeproj` fails to load with a `missing field`
   error. Pin this in a regression test (load the smallest project
   fixture in `tests/fixtures/` and assert success).
2. **Migration runs ONCE.** `after_load_migrate` checks
   `self.layers.layers.is_empty()` before seeding. If a user opens
   a project, deletes all layers (Sprint 17+), then re-opens, the
   migration must NOT re-seed.
3. **Mask byte-size budget.** 16-SMU map × 8 layers × 8192² × 1 byte
   = **512 MB** resident. Sprint 15 ships the flat allocation
   intentionally for now, but the `Project::after_load_migrate`
   should refuse to seed more than 4 layers (the splat-config
   migration max) and the bake should log a `warn!` over 256 MB
   total mask memory so the user gets a heads-up before Sprint 16's
   COW lands.
4. **BMP dimensions must be a multiple of 1024 per side.** PyMapConv
   infers `mapx = width / 8`. `MapSize::texture_dims` (`512 × SMU`)
   guarantees this for `SMU ≥ 2`. The bake function should
   `debug_assert!` it.
5. **DXT1 compression downstream.** The user's high-res composite
   gets DXT1-compressed by PyMapConv (BAR mandates
   `compressionType=1, tileSize=32`, SRS §1.2). Expect chroma drift
   on gradients. The lint pass in Sprint 19 (lint pass) will surface this if
   it gets bad; Sprint 15 just documents the caveat in ADR-038.
6. **Wallpaper sampling, not edge-clamp.** When a layer's transform
   scales the texture so it doesn't cover the full diffuse, the
   compositor MUST tile in both directions (modulo). Edge-clamp
   produces a "stretched smear at the seams" look the user explicitly
   rejected.
7. **Mirror flags compose with rotation.** When both `mirror_x` and
   `rotation_rad` are set, apply mirror BEFORE rotation in the
   sample math. Otherwise rotating a mirrored texture produces the
   wrong orientation. Pin this in a unit test
   (`bake_mirror_then_rotate_matches_reference`).
8. **DNTS-bound layers still contribute to the diffuse.** They are
   not "DNTS-only" — they're regular layers that ALSO emit to the
   splat distribution at export time. The diffuse bake treats them
   identically; the DNTS hookup happens in Sprint 17.
9. **`splat_config` is the source of truth for runtime DNTS until
   Sprint 17.** Sprint 15's migration COPIES the bindings into the
   layer stack but does NOT delete them from `splat_config`. Both
   live side-by-side for one sprint; Sprint 17 retires `splat_config`
   when the new emission path takes over.
10. **The base BMP fallback stays.** Test harnesses that build a
    `Project` programmatically (e.g., `barme-pipeline::examples::
    build_smoke`) currently rely on `synth_biome_bmp`. Sprint 15's
    bake falls back to it when the layer stack is empty — DO NOT
    delete it.

## Step 7 — Exit criteria

- 4 commits on `main`: data model, project + diff, export hookup,
  rollup.
- 1 devlog folder filled (`devlog/stage-1-layers-data-model/`).
- 1 checkbox ticked in `phase-3-plan.md` (D8).
- ADR-038 in `docs/DECISIONS.md` with the schema + migration rules
  + bake performance target inline.
- SRS / ROADMAP STATUS UPDATEs (layer stack data model shipped,
  bake replaces biome ramp on .sd7 export, paint UI gated on
  Sprint 16, Layers panel gated on Sprint 17).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
  && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Open an existing project saved before Sprint 15 → loads cleanly,
    `Project.layers.layers.len() == 4` (one per bound DNTS slot),
    `splat_config` retained.
  - Open a fresh project (wizard → Create) → `Project.layers.layers.len() == 1`,
    that one layer's source matches the biome's base slot.
  - Run `cargo run -p barme-app` → editor launches, no UI changes
    visible (Sprint 15 is data-only).
  - Run `cargo run -p barme-pipeline --example build_smoke` → `.sd7`
    builds; the .smt texture is NOT a biome ramp (the
    `build_smoke` example uses an empty-stack `Project`, so this
    still falls back to `synth_biome_bmp` — note in devlog as
    intentional). Switch one line in `build_smoke.rs` to seed a
    base layer and re-run → .smt now shows the wallpaper-tiled
    grass-meadow diffuse.
  - Open the new project in BAR → map loads with a recognizable
    grass-textured ground (not the blurry blue-green-grey biome
    ramp).
- Final devlog log summarizes what shipped + "Sprint 16 = paint
  viewport + GPU composite + tiled-COW masks (D9)" handoff note.

Start by running `git status`, then read the existing
`synth_biome_bmp` at `crates/barme-app/src/launcher.rs:253-300` so
you know exactly what you're replacing. Begin with Commit 1 (data
model + bake); Commit 2 depends on the schema; Commit 3 depends on
both.
