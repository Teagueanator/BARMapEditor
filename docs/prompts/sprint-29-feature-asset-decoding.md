# Sprint 29 — Feature asset decoding (decals + S3O) (R5)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 29** — the fifth renderer-parity sprint (R5 in
ROADMAP numbering). Today, placed features (trees, rocks, wreckage,
props) render as flat category-coded glyphs via the existing GPU
marker pipeline (Sprint 13 / ADR-037). The user can see WHERE each
feature is + its METAL value, but cannot see WHAT it looks like.

This sprint closes that visual gap. After this sprint:

- **Stock features render as visually correct sprites** via decals
  (Phase A — top-down accurate but flat at oblique angles), or
- **Stock features render as accurate 3D meshes** via S3O parsing
  (Phase B — full visual parity at any camera angle).

**Phase A is required**; Phase B is the stretch goal. The user
decides at sprint kickoff whether Phase B fits the sprint or
gets its own follow-up sprint (Sprint 29b).

**Prerequisites:**
- Sprint 25 (terrain shader parity) MUST be ticked. Features depth-
  test against the upgraded terrain.
- Sprint 28 (atmosphere + fog) MUST be ticked. Features receive
  fog from the atmosphere uniforms.
- Sprint 13 (ADR-037) GPU marker pipeline is the rendering
  foundation Phase A extends with `MarkerShape::TexturedSprite`.

**Reference clone of RecoilEngine:** `/home/teague/code/RecoilEngine`.
Critical files:
- `rts/Rendering/Models/S3OParser.cpp` — S3O binary format
  reference (~1500 LoC of C++).
- `rts/Rendering/Models/3DOParser.cpp` — legacy 3DO format.
- `rts/Rendering/Models/ModelRenderer*.cpp` — instance rendering.

**Reference BAR feature repo:** clone
`github.com/beyond-all-reason/mapfeatures` to
`~/code/Beyond-All-Reason/mapfeatures` if not present.

**Empirical baselines from 2026-05-20 research** (do NOT re-derive):
1. `assets/mapfeatures_catalog.json` uses community names
   (`pinetree`, `agorm_pine1`, etc.) that match the upstream
   `mapfeatures` repo.
2. BAR's `unittextures/decals_features/*.dds` has 680 sprites but
   **0 of 34 catalog entries match any decal directly** (decals
   are for units/structures/wrecks, not community mapfeatures).
3. BAR has **NO tree decals** — trees render via the engine's
   procedural tree shader. Even with a perfect catalog mapping,
   tree decals don't exist.
4. Real `.s3o` files live at
   `~/code/Beyond-All-Reason/objects3d/` (e.g.
   `fir_tree_small.s3o`, `rocks30/rocks30_def_NN.s3o`). Format
   is documented in S3OParser.cpp.
5. The `image` crate is `default-features = false, features =
   ["png", "bmp"]`. Adding `tga` is one-line; `dds` needs `bcdec_rs`
   or `image_dds` for BC1/3 compressed textures.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §1 (heap
   budget — 16-SMU map with 2000 features at full S3O blows the
   iGPU budget; thumbnails are the safe path), §11 (pink-map on
   rename — S3O texture paths apply).
3. `/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`
   — Sprint 20 section (original numbering) = this Sprint 29.
4. `assets/mapfeatures_catalog.json` — current state.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs`
   — search "FeatureCatalog" / "resolved_visual" /
   "ResolvedFeatureVisual" / `inspector_feature`. These are the
   integration points this sprint extends.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/markers.rs`
   + `crates/barme-app/src/markers.wgsl` — the pipeline that
   gains the `TexturedSprite` variant.
7. `/home/teague/code/RecoilEngine/rts/Rendering/Models/S3OParser.cpp`
   — if Phase B in scope.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-29-feature-asset-decoding
```

## Step 3 — Scope

Pick Phase A only OR Phase A + B. Phase A is mandatory; Phase B
is the stretch goal.

### Phase A — Decal-based sprites (mandatory; ~1 week)

#### 1. Workspace deps + DDS decode

Workspace `Cargo.toml`:
```toml
image = { version = "0.25", default-features = false, features = ["png", "bmp", "tga"] }
bcdec_rs = "0.4"  # for BC1/3 DDS decode
```

`crates/barme-app/src/feature_decals.rs` (new): a thin wrapper
over `bcdec_rs` that decodes `.dds` files to RGBA8.

#### 2. Vendor the decal subset

Mirror the texture-pack pattern (`scripts/fetch-textures.sh`).
New script: `scripts/fetch-feature-decals.sh`. SHA-pinned, idempotent.

For each of the 34 catalog entries that has an available decal:
- Identify the upstream `mapfeatures` decal path.
- Vendor under `tools/feature-decals/<entry_name>/<filename>.dds`.

For entries without an available decal (trees mostly): fall back
to the Sprint 19 category glyph (no change).

#### 3. Extend `markers.wgsl` with TexturedSprite

```wgsl
const SHAPE_CIRCLE: u32 = 0u;
// ... existing shapes ...
const SHAPE_TEXTURED_SPRITE: u32 = 5u;

@fragment
fn fs_main(@location(0) uv: vec2<f32>, @location(1) shape: u32, @location(2) tex_layer: u32) -> @location(0) vec4<f32> {
    if (shape == SHAPE_TEXTURED_SPRITE) {
        return textureSampleLevel(decal_array, sampler_lin, vec3<f32>(uv, f32(tex_layer)), 0.0);
    }
    // ... existing shape paths ...
}
```

Add a per-instance `texture_layer: u32` field to `MarkerInstanceGpu`
(currently `[f32; 4]` pad — there's room). The decals load into a
`texture_2d_array<f32>` of fixed dimension (e.g. 128² × N layers).

#### 4. Build `feature_decal_registry`

At app startup (`barme-app/src/main.rs::App::new`):
```rust
let mut registry = FeatureDecalRegistry::default();
for entry in feature_catalog().entries() {
    if let Some(decal_path) = find_decal_for(entry.name) {
        let layer = registry.load_decal(&decal_path)?;
        registry.insert(entry.name.clone(), layer);
    }
}
```

Falls back to the Sprint 19 category marker on miss.

#### 5. Inspector polish: hover thumbnail

`inspector_feature` (main.rs:6646): the per-feature row gains
a 32×32 thumbnail preview (from the decal cache) next to the
name. On miss, show the category glyph.

#### 6. ADR

`/home/teague/code/BARMapEditor/docs/DECISIONS.md`:

```
## ADR-04X — Feature decal sprite atlas (Sprint 29 / R5)

Status: ADOPTED 2026-05-XX
...
```

### Phase B — S3O rendering (stretch; 2-3 weeks)

#### 1. New crate `barme-render-s3o`

```
crates/barme-render-s3o/
  src/
    parser.rs        // S3O binary parsing
    mesh.rs          // GPU mesh upload
    instance.rs      // instance buffer per feature
    lib.rs
```

The S3O parser is a non-trivial port of `S3OParser.cpp`. Skim
the C++ for:
- 12-byte header magic `"Spring unit"` + version + fileSize.
- Piece tree: each piece with vertices + indices + child pieces.
- Texture name pointers (resolved to `~/code/Beyond-All-Reason/objects3d/`
  or vendored mirror).

#### 2. Top-down thumbnail render pass

For each unique S3O file:
1. Load + parse.
2. Render at 128² RGBA, top-down orthographic, neutral lighting.
3. Cache to `$XDG_CACHE/barme/feature_thumbnails/<sha256>.png`.
4. Bind to the marker texture array.

Renders ONCE per unique feature; cached output reused.

#### 3. Full-perspective rendering (deep stretch)

Stretch goal beyond cache: render S3O models at full 3D scale
+ rotation in the viewport. New `model.wgsl` + instance pipeline.

**Caveat**: 2000 features at full S3O = millions of triangles.
LOD-by-distance is needed. Stage-2 polish realistically.

#### 4. Multiple ADRs

Parser, render pass, cache schema, GPU memory budget. Each
gets a separate ADR.

### Cross-phase work

#### Catalog enrichment

Small commit:
- Parse BAR's `features/*.lua` Lua files to extract exact `metal`
  values per feature name. Overwrite the coarse category defaults
  in `mapfeatures_catalog.json`.
- Add `s3o_path` field per entry. Phase B uses these as direct
  lookups.

#### Tooltip + help center integration

- Hover surfaces FeatureDef.name + metal value (already
  exists, but now richer thumbnail).
- Sprint 22's help center gains an article on the feature
  decal/S3O system + how to add custom mapfeatures (Stage 2).

### Validation fixtures

Add `assets/parity-fixtures/feature-zoo/` — a 4-SMU project with
30+ features scattered, one of each catalog category. Side-by-
side editor vs BAR screenshot. Acceptance: features placed at
correct positions + visually recognisable.

**Platform-portability checklist**:
- `bcdec_rs` is pure Rust, cross-platform.
- `texture_2d_array` is standard WGSL.
- S3O parser is pure Rust (Phase B) — no platform concerns.
- File-system path resolution: handle case-sensitive vs
  case-insensitive (Windows / macOS).

### Rollup

STATUS UPDATEs in SRS / ROADMAP (R5 done, renderer-parity 4/8 if
Phase A only, 5/8 if both). closing devlog log. "Sprint 30 =
directional shadows" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on decal cache hit/miss
+ load times; `warn!` on missing decals; `trace!` on per-frame
instance count.

## Step 5 — Out of scope

- **Animated features** (grass sway, water surface, geo plumes)
  — geo plumes already render via lines (Sprint 13); animation is
  Sprint 26's work for water surface.
- **Feature LOD** — at minimap zoom levels we already show dots.
- **Importing custom feature packs from a user-supplied dir** —
  Stage 3 stretch.
- **Lighting on feature meshes** (Phase B writes flat colour) —
  Sprint 30 adds shadows; full Lambert is Sprint 32 polish.
- **Mapfeatures auto-catalog generation** — separate small sprint
  (deferred); the manual 34-entry catalog is sufficient for now.

## Step 6 — Critical pitfalls (read twice)

1. **Phase scope decision is mandatory before starting**. Talk
   with the user at kickoff. If you ship Phase A only, the
   commits + ADR-04X cover that; if Phase B in scope, the work
   spans this sprint + likely Sprint 29b. Don't half-ship Phase B.

2. **Licensing**: vendor only the CC0 / clearly-licensed decals.
   The `mapfeatures` LICENSE file must be audited. If a decal is
   ambiguous, drop it — don't ship with murky licensing.

3. **16-SMU memory budget** (PITFALLS §1): 128² × 128-feature
   thumbnail atlas is ~8 MB RGBA. Full 512² mipmapped sprites
   blow the budget; stick to 128².

4. **Phase B model count**: BAR has ~200 unique stock features
   (across categories). Caching at 128² → ~3 MB total cache
   for thumbnails. Sustainable. Live S3O rendering of 2000
   features is NOT sustainable without LOD.

5. **S3O texture path resolution**: S3O files reference texture
   names that map to BAR's `unittextures/` directory. We
   don't vendor that directory. For Sprint 29 Phase A, fall back
   to a default texture for any missing path. Phase B requires
   a vendoring decision — defer to a separate ADR.

6. **`bcdec_rs` vs `texpresso`**: `texpresso` is in our workspace
   already for splat compression. It has decode support too,
   but `bcdec_rs` is more focused. Either works; document the
   choice.

7. **DDS files come in BC1, BC3, BC5 formats. Cover them all.**
   Future-proof the decoder; trees may be BC1, rocks BC3.

8. **Catalog `s3o_path` field**: optional. Entries without an
   S3O path fall back to category glyphs even in Phase B.
   Sprint 29 doesn't force the user to vendor every model.

9. **Thumbnail cache invalidation**: keyed by `sha256(s3o_file)`.
   If the user replaces the upstream model, the hash changes
   and a new thumbnail is generated. Garbage-collect old hashes
   when no catalog entry references them.

10. **Wait for `mapfeatures` upstream manifest**: if the upstream
    repo adds a JSON manifest with metal + category + s3o_path,
    Sprint 29's catalog work becomes trivial. Open a PR upstream
    if not present; in the meantime, manual curation lives.

## Step 7 — Exit criteria

**Phase A only**:
- 5+ commits on `main`: workspace deps, decal fetch, marker
  shape extension, registry build, inspector polish, ADR + rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R5 Phase A done; renderer 4/8).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Feature-zoo fixture renders 30 features as decals where
    available + category glyphs as fallback.
  - Inspector hover shows thumbnail.
  - 16-SMU project with 200 features → < 50 MB RAM increase.

**Phase B (if in scope)**:
- 10+ commits on `main` (parser, thumbnail render, cache, ADR-X,
  ADR-Y).
- New crate `barme-render-s3o` lands.
- Smoke test:
  - Feature-zoo fixture renders 30 features as accurate 3D
    thumbnails or models.
  - Cache hits on second editor launch.
  - 2000-feature project renders without OOM (LOD or thumbnail-
    only).

Final devlog: summary + Phase split notes + "Sprint 30 =
directional shadows" handoff.

Start by asking the user about Phase B scope. If Phase A only,
the work is mechanical: vendor decals, lift the marker shape,
wire the registry, polish the inspector. Phase B is a multi-week
parser project; if it doesn't fit, defer to Sprint 29b.
