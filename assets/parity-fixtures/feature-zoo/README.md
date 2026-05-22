# Feature-zoo — Sprint 29 / R5 / Phase A + Phase B parity fixture

**Purpose:** Sprint 29 / R5 reference for the feature decal sprite
atlas. Sprint 29 / ADR-046 (Phase A) shipped per-family decal
sprites; Sprint 29b / ADR-047 (Phase B) added per-entry 3D
thumbnails baked from the upstream `.s3o` files. Verifies that
both passes populate the atlas correctly and that
`resolved_visual` walks the priority chain (per-entry thumbnail →
per-family decal → category glyph → unknown fallback).

## What this fixture exercises

- **All 16 vendored families render as TexturedSprite** — one
  representative variant per family placed at a grid cell on a
  4-SMU map.
- **Fallback families render as category glyphs** — kapok (no
  upstream diffuse), rocks30 / tombstone / xmascomwreck /
  geovent (`source: bar` or `source: engine`; no diffuse vendored)
  fall back to `category_visuals[<cat>].shape` glyphs.
- **Inspector thumbnails** — F7 picker shows each entry with its
  32² thumbnail (where a diffuse loaded) or a category-tinted
  square (fallback).
- **Atlas layer count** — verify `MARKER_DECAL_LAYERS = 32`
  comfortably holds the 16 Phase A diffuses with headroom for
  future families. Tracing log at app start should report
  `loaded = 16, missing = 5, overflow = 0`.
- **Sprite scaling** — `resolved_visual` multiplies the
  category-glyph `radius_px` by 3 for TexturedSprite, so a tree
  sprite at radius 7 px × 3 = 21 px ≈ 42×42 quad. Confirm
  sprites are legibly larger than glyphs at default camera.

## Suggested project settings

- **Map size:** 4 × 4 SMU (`MapSize::SMU { x: 4, z: 4 }`). 256² m
  playable area; small enough to see every feature at default
  camera framing.
- **Min height:** 0 elmos (no flooded basin — keep the focus on
  feature visibility, not terrain).
- **Max height:** 80 elmos (gentle relief; features don't get
  buried by terrain occlusion).
- **MapInfo:** `MapInfo::bar_default()` — no per-feature mapinfo
  knobs at this stage.

## Feature placement layout

A 4×4 SMU map is 1024×1024 elmos (8 elmos / heightmap pixel ×
128 pixels per SMU side × 4 SMU). One feature per ~120 elmo cell
yields an 8×8 = 64-cell grid; 30 features cover ~half the cells
with spacing for visual inspection.

| Cell | x (elmos) | z (elmos) | Family             | Variant                         | Category | Source       |
|------|-----------|-----------|--------------------|---------------------------------|----------|--------------|
|  1   | 128       | 128       | ad0_aleppo2        | ad0_aleppo2_dense_med_l         | trees    | mapfeatures  |
|  2   | 256       | 128       | ad0_banyan         | ad0_banyan_1                    | trees    | mapfeatures  |
|  3   | 384       | 128       | ad0_cedar_atlas    | cedar_atlas_2__cedar_atlas_2    | trees    | mapfeatures  |
|  4   | 512       | 128       | ad0_fir            | fir_tree_tall_1__tree_fir_tall_3 | trees   | mapfeatures  |
|  5   | 640       | 128       | ad0_senegal        | ad0_senegal_1                   | trees    | mapfeatures  |
|  6   | 768       | 128       | allpinesb_ad0      | allpinesb_ad0_green_c_l         | trees    | mapfeatures  |
|  7   | 896       | 128       | birchtree          | euro_birch_tree_03_1            | trees    | mapfeatures  |
|  8   | 128       | 256       | kapok              | kapok1                          | trees    | **fallback** |
|  9   | 256       | 256       | agorm_rock         | agorm_rock1                     | rocks    | mapfeatures  |
| 10   | 384       | 256       | agorm_rock         | agorm_rock3                     | rocks    | mapfeatures  |
| 11   | 512       | 256       | pdrock             | pdrock1                         | rocks    | mapfeatures  |
| 12   | 640       | 256       | pdrock             | pdrock5                         | rocks    | mapfeatures  |
| 13   | 768       | 256       | rocks30            | rocks30_green_01                | rocks    | **fallback** |
| 14   | 896       | 256       | rocks30            | rocks30_green_02                | rocks    | **fallback** |
| 15   | 128       | 384       | anemone            | anemone1                        | props    | mapfeatures  |
| 16   | 256       | 384       | anemone            | anemone4                        | props    | mapfeatures  |
| 17   | 384       | 384       | cycas              | cycas1                          | props    | mapfeatures  |
| 18   | 512       | 384       | cycas              | cycas4                          | props    | mapfeatures  |
| 19   | 640       | 384       | mushroom_orange    | mushroom01                      | props    | mapfeatures  |
| 20   | 768       | 384       | mushroom_orange    | mushroom05                      | props    | mapfeatures  |
| 21   | 896       | 384       | mushroom_purple    | mushroom11                      | props    | mapfeatures  |
| 22   | 128       | 512       | mushroom_purple    | mushroom15                      | props    | mapfeatures  |
| 23   | 256       | 512       | mushroom_tan       | mushroom21                      | props    | mapfeatures  |
| 24   | 384       | 512       | mushroom_tan       | mushroom25                      | props    | mapfeatures  |
| 25   | 512       | 512       | pedro              | pedro1                          | props    | mapfeatures  |
| 26   | 640       | 512       | pedro              | pedro4                          | props    | mapfeatures  |
| 27   | 768       | 512       | peyote             | peyote1                         | props    | mapfeatures  |
| 28   | 896       | 512       | peyote             | peyote3                         | props    | mapfeatures  |
| 29   | 128       | 640       | tombstone          | armstone                        | props    | **fallback** |
| 30   | 256       | 640       | tombstone          | corstone                        | props    | **fallback** |
| 31   | 384       | 640       | xmascomwreck       | xmascomwreck                    | wreckage | **fallback** |
| 32   | 512       | 640       | xmascomwreck       | gingerbread                     | wreckage | **fallback** |
| 33   | 640       | 640       | geovent            | geovent                         | geo      | **fallback** |

**Result:** 33 features, 17 distinct families (16 with vendored
diffuse + 1 with explicit-fallback layer for kapok), 5 categories
exercised.

## How to use (manual smoke until Sprint 36 automates)

1. Boot a fresh 4-SMU project with the settings above
   (`File → New project → 4×4 SMU`).
2. Open **F7 (Features)**. Confirm the picker shows 32×32
   thumbnails next to each entry. Spot-check: every entry whose
   family's `source = "mapfeatures"` (with a non-null
   `diffuse_texture`) shows a real diffuse; entries with
   `source = "bar"` / `"engine"` show a category-tinted square.
3. Place each of the 33 features at its grid position above (LMB
   in the canvas after selecting the entry).
4. Orbit to a top-down view, then to a 35° pitch.
   - Mapfeatures rows: 28 of 33 features should render as
     diffuse-textured sprites (the 16 distinct families across
     trees / rocks / props × the picked variants).
   - Fallback rows: 5 of 33 features should render as their
     category glyph (kapok = green triangle; rocks30 = grey
     disc; tombstone = blue ring; xmascomwreck = orange ringed-
     fill; geovent = amber triangle).
5. Check the tracing output:
   ```
   feature decal registry populated loaded=16 missing=5
                                    overflow=0
                                    atlas_layers_used=16
                                    atlas_layers_total=32
   ```
6. Save the project to `out/feature-zoo.barme.json` (or whatever
   path; the file is gitignored). Reopen — features still
   resolve, sprites still load (cache hit on the same
   `tools/feature-decals/<family>/diffuse.tga` paths).

## Acceptance

- 16 / 21 catalog families render as decal sprites (per
  `source = "mapfeatures"` with `diffuse_texture` non-null).
- 5 / 21 catalog families render as the category glyph (kapok,
  rocks30, tombstone, xmascomwreck, geovent).
- F7 picker shows 32² thumbnails on every populated row + a
  category-tinted square on every fallback row.
- Tracing log shows `loaded = 16, missing = 5, overflow = 0` at
  app start.
- 33 features placed → < 50 MB RSS increase over the empty
  project baseline. Atlas footprint ≈ 16 × 64 KB = 1 MB; egui
  thumbnail textures ≈ 16 × 64 KB = 1 MB; the rest is per-
  instance MarkerInstanceGpu (33 × 48 B ≈ 2 KB; trivial).

## Sprint 29b / Phase B addendum (2026-05-22)

Phase B replaced Phase A's per-FAMILY decals with per-ENTRY 3D
thumbnails. Run `scripts/fetch-feature-s3o.sh` first; that
vendors 85 catalog entries from upstream `mapfeatures/objects3d/`
into `tools/feature-s3o/<entry>.s3o`. At app start
`populate_decal_registry`:

1. **Per-entry pass.** For every entry with a vendored .s3o:
   - compute `sha256(s3o_bytes)` → cache lookup at
     `$XDG_CACHE_HOME/barme/feature_thumbnails/<sha>.png`;
   - on miss, parse via `barme_render_s3o::parser::parse_s3o`
     and bake via `barme_render_s3o::thumbnail::bake_thumbnail`
     (CPU rasteriser — top-down ortho, Lambert lighting, family
     diffuse as the surface colour, or synthetic mid-grey when
     the family has `diffuse_texture: null`);
   - upload to next atlas layer + stamp `entry.decal_layer` +
     `entry.egui_thumbnail`.
2. **Per-family Phase A fallback.** For every family with
   `diffuse_texture` that still has uncovered entries, run the
   legacy Phase A decal upload (unchanged from Sprint 29).

### Expected tracing on a fresh launch

```
feature decal registry populated
  phase_b_loaded=85 phase_b_cache_hits=0 phase_b_missing_s3o=8
  phase_b_parse_errors=0 phase_a_loaded=0 phase_a_missing=0
  overflow=0 atlas_layers_used=85 atlas_layers_total=128
```

Second launch should show `phase_b_cache_hits=85`,
`phase_b_loaded=0` — the cache PNGs make cold start ~10× faster.

### Expected visual

Each Phase A test entry now shows a SPECIFIC variant's 3D
silhouette in the F7 picker thumbnail (e.g. `pdrock1` shows the
small rock; `pdrock5` shows the larger one), and in the
viewport each placed feature's sprite reflects its own
geometry. Eight catalog entries (rocks30×2 + tombstone×3 +
xmascomwreck×2 + geovent) still fall to category glyphs.

### Out of scope (Phase B remains — Sprint 29c+)

- **Live mesh rendering in the viewport** — per-feature 3D
  meshes drawn at runtime with LOD-by-distance. The Sprint 29
  brief's Path 2 option; deferred until the user requests it.
- **BAR-side feature support** — rocks30, tombstone, xmascomwreck,
  geovent live in BAR's `luarules/featureDefs/` or are engine-
  internal. A Phase C fetch script + parser would lift them
  onto the Phase B path.
- **Per-feature lighting in the viewport** — Phase A/B sprites
  are unlit (thumbnails baked with Lambert; viewport sprite is
  flat texture). Shadows arrive in Sprint 30.
- **Animated features** — none of the upstream families need
  animation; if Sprint 21+ ships a `cycas`-style sway pass, the
  fixture grows a verification step.
