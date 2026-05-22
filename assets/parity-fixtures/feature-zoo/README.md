# Feature-zoo — Sprint 29 / R5 / Phase A parity fixture

**Purpose:** Sprint 29 / R5 / ADR-046 reference for the feature
decal sprite atlas. Verifies that each upstream-`mapfeatures`
family lands on its own atlas layer + renders as a `TexturedSprite`
in the viewport, with category-glyph fallback for the families
that lack an upstream diffuse (kapok, rocks30, tombstone,
xmascomwreck, geovent).

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

## Out of scope (deferred to Sprint 29b — Phase B)

- **3D meshes** — S3O parsing + thumbnail render passes via a
  new `barme-render-s3o` crate. Sprint 29b kickoff brief at the
  end of this sprint's devlog rollup.
- **Per-variant catalog** — upstream enumerates ~280 variants
  across the 17 families; this fixture exercises the
  representative subset. The "auto-catalog generation" sprint
  (separate / deferred) expands variant coverage.
- **Per-feature lighting** — Phase A sprites are unlit. Lambert
  + shadows arrive in Sprint 30 / 32.
- **Animated features** — none of the upstream families need
  animation; if Sprint 21+ ships a `cycas`-style sway pass, the
  fixture grows a verification step.
