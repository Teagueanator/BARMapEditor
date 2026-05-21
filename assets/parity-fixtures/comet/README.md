# Comet Catcher Remake — renderer-parity fixture

**Purpose:** The Sprint 25 / R1 / ADR-043 manual smoke test for the
`SMFFragProg.glsl` port. After the editor's terrain shader has been
rewritten to mirror the engine, loading this fixture and visually
comparing against BAR's render of the same map should show the editor
reproducing BAR's lighting, normal mapping, DNTS detail blending, and
per-fragment specular highlights at editor camera distances (2-8 SMU).

## Provenance

- **Map:** Comet Catcher Remake v1.8 by IceXuick (original by NoiZe).
- **Source:** the user-local extracted copy under
  `scratch/bar-maps/extracted/comet/`. The `.sd7` archive is at
  `scratch/bar-maps/originals/comet_catcher_remake_1.8.sd7`.
- **Why Comet?** It exercises every Sprint-25 shader feature in one
  map:
  - `splatDetailNormalDiffuseAlpha = 1` — exercises the FINDINGS §7.3
    diffuse-in-alpha path that adds `clamp(splat_detail_normal.a, -1,
    1)` to the diffuse base.
  - 4 distinct DNTS slot normals (`pebbles_49`, `sandpebbles_NORM`,
    `earth_NORM`, `crystal_245`) — exercises the per-channel
    `tex_scales` UV streams (`splats.TexScales = {0.004, 0.007, 0.003,
    0.0018}`).
  - `specularTex = "specular.png"` — exercises the per-fragment
    specular exponent (FINDINGS §7.6).
  - `detailNormalTex = "normalmap.png"` — exercises the R+A base-normal
    decode (FINDINGS §7.5).

## Provenance — what's in the user-local source

```
scratch/bar-maps/extracted/comet/
├── maps/
│   ├── CCRXR.smf                       # heightmap binary
│   ├── CCRXR.smt                       # baked SMT tile diffuse
│   ├── normalmap.png                   # 2048×1024 RGB base normal
│   ├── specular.png                    # 2048×1024 RGBA specular
│   ├── splat_distr.png                 # 2048×1024 RGBA distribution
│   ├── pebbles_49_highpass_dnts.tga    # DNTS slot 1 normal
│   ├── sandpebbles_NORM_smooth.tga     # DNTS slot 2 normal
│   ├── earth_NORM.tga                  # DNTS slot 3 normal
│   ├── crystal_245_highpass_dnts.tga   # DNTS slot 4 normal
│   ├── detailtexblurred.bmp            # detail texture
│   ├── grass_NORM.dds                  # grass normal (Sprint 34)
│   ├── mini.dds + mini.png             # minimap
│   └── ...
├── mapinfo.lua                          # the canonical mapinfo
└── ...
```

SMF header bytes (decoded for reference; tracked in
`docs/research/source-audit-2026-05-18/FINDINGS.md` §2):

| field          | value      |
|----------------|------------|
| magic          | `"spring map file"` |
| version        | 1          |
| mapid          | 509        |
| mapx           | 1024       |
| mapy           | 768        |
| squareSize     | 8          |
| texelPerSquare | 8          |
| tileSize       | 32         |
| minHeight      | -50.0      |
| maxHeight      | 100.0      |

The mapinfo overrides `smf.minheight = 100` and `smf.maxheight = 450`
(FINDINGS §1.8 — `smf.{minHeight,maxHeight}` override the SMF header
when present). So the **editor preview range is `[100, 450]` elmos.**

So Comet is a **16 × 12 SMU** map (1024 / 64 × 768 / 64), heightmap
`1025 × 769` `u16`.

## What this directory ships in the tracked repo

- `README.md` — this file.
- `bar-reference/` — empty; populate manually with BAR screenshots
  per the procedure below. Files dropped here are gitignored as
  `*.png` if and only if they land under `/assets/fixtures/`; this
  subdir is `/assets/parity-fixtures/comet/bar-reference/` which is
  NOT in the gitignore, so screenshots that land here CAN be
  committed. Keep them ≤ 1 MB each (downsample at capture time).

What's **NOT** shipped:

- The Comet `.sd7` and its baked textures. Binary `.smf` / `.smt` /
  `.sd7` are globally gitignored. The user's local clone of the BAR
  map archive at `scratch/bar-maps/extracted/comet/` is what the
  fixture loader consults; the loader stays useful for any developer
  who has placed the same archive in that path.
- A heightmap parsed from `CCRXR.smf`. Sprint 25's fixture loader
  uses a *synthesised* heightmap matching Comet's `(1025, 769)` dims
  with a procgen "crater + ridges" shape that approximates Comet's
  silhouette. The actual binary heightmap parser ships with Sprint 36
  (the ΔE-validation harness), which has to solve the headless-render
  question anyway.

## Manual smoke procedure

The Sprint 25 acceptance test is a **human visual review** at editor
camera distances 2-8 SMU. Until Sprint 36 ships the ΔE automation,
follow this procedure:

1. **Capture BAR reference screenshots** (one-time, before the first
   review):

   ```bash
   # Launch BAR with the Comet Catcher map directly.
   recoil --isolation \
          --gen-screenshot   \
          --start-script ...   # see RecoilEngine's screenshot examples
   ```

   Or capture in-game manually:
   - Open BAR, host a Skirmish on Comet Catcher Remake.
   - Position the camera at a canonical angle (top-down, 35° tilt,
     grazing) — use a fixed seed for reproducibility.
   - Take a screenshot via `F12` (in-engine binding for screenshots).
   - Save the three screenshots as
     `bar-reference/top-down.png`, `35-tilt.png`, `grazing.png`.

   Commit these screenshots once captured. Re-capture only if Recoil
   ships a renderer change that affects parity.

2. **Open Comet's mapinfo values in the editor** via the parity
   fixture:

   ```bash
   cargo test -p barme-app comet_catcher 2>&1 | grep "fixture"
   ```

   This produces a `Project` shaped like Comet (matching SMU dims,
   min/max height, lighting block, splats block, DNTS layer
   bindings). The test asserts the project's shape but does NOT
   render — rendering at editor scale needs a real wgpu device.

3. **Run the editor with the fixture loaded** (manual, until a
   proper "load reference fixture" UI ships in Sprint 36):
   - Copy the Comet `.sd7` into a fresh editor project workspace.
   - Open in the editor.
   - Position the camera at the same three canonical angles.
   - Compare side-by-side against `bar-reference/*.png`.

4. **Drift list.** Record any visual divergences in the Sprint 25
   devlog's `roadblocks.md`. Sprint 26-35 close them per the
   renderer-parity arc.

## Acceptance bar

Sprint 25 acceptance is *visually indistinguishable at 2-8 SMU
camera distance, by human review*. Specifically:

- The diffuse base reproduces BAR's terrain colour family.
- DNTS detail visibly blends across the 4 slot normals (the 4
  detail textures' fingerprints visible on close inspection).
- Specular highlights track the sun direction
  `lighting.sunDir = (1.2, 0.92, -0.79)` per fragment — NOT a flat
  intensity across the terrain.
- Slope-driven hue shift from the base normal map is visible on
  ridge edges.

Sprint 36 (parity-validation) ships a ΔE harness that mechanises
this acceptance against a 3-map suite (Comet + All That Simmers +
one void-water map).
