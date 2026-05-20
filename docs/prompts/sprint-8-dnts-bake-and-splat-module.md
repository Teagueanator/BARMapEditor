# Sprint 8 — DNTS bake pipeline + splat module (D2, D3)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 8** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship **D2 + D3** — the DNTS bake pipeline (normal-prep +
Compressonator + DDS emit, with a configurable Y-flip for non-GL
sources) and the `barme-core::splat` module
(distribution + brush trait + dirty-rect pattern). Together they form the
data + pipeline foundation for splat painting; D4 (Sprint 9) wires the
shader, D5 (Sprint 9) wires the UI.

**Prerequisites:** Sprint 7 (D1) MUST be ticked — the palette and fetch
script need to exist before the bake pipeline has inputs. Sprints 5 and 6
*may or may not be done*; this sprint doesn't depend on them.

This sprint produces no visible feature on its own — D2 + D3 are
pipeline + data. The visible payoff lands in Sprints 9 / 11 (shader, UI,
emission wiring).

**UX context (ADR-035, already shipped):** the editor's left tool
strip already contains a **Splat paint** tile (keyboard `T`), and the
right inspector renders a scaffolding panel
(`crates/barme-app/src/main.rs::inspector_splat`) with a 4-row RGBA
layer list, channel chips, brush-mode buttons, and radius / strength /
spacing sliders backed by `App::splat_state: SplatState`. That state
is intentionally a parallel struct — Sprint 9 D5 will replace it with
one driven by `SplatDistribution` (from your D3) + a new
`Project.splat_config`. **You do not need to touch any of this** —
the Phase-7 stub is decoupled from the bake pipeline + brush trait
you ship here. But: align your `SplatChannel` enum's order with the
inspector's R / G / B / A row order so D5 doesn't need a translation
layer, and keep `SplatBrush::id()` returning short kebab strings
(`paint`, `erase`, `smooth`) so the inspector's brush-mode buttons
can match them by id later.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (DNTS lore: `splatDetail
   NormalTex` requires `specularTex`), §2.1 (texture-pipeline pitfalls
   #1, #2, #3, #6), §3.4 (architecture).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable rules.
4. `/home/teague/code/BARMapEditor/devlog/README.md`.
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md`.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` —
   read D2 and D3 in full. Skim D4 / D5 / D6 to understand what consumes
   your output.
7. `/home/teague/code/BARMapEditor/docs/research/textures/Gemini BAR Editor Texture Pack Scoping.md`
   — section on Y-flip (Recoil's OpenGL tangent space). **Read alongside
   source-audit FINDINGS §7.4**: the engine uses OpenGL convention, so
   the Y-flip is needed ONLY when the source normal is DirectX (e.g.
   Beherith's `_flipped.dds` files, originally Substance / Quixel
   exports). The D1-shipped starter pack uses ambientCG `_NormalGL.png`
   sources, which are ALREADY OpenGL — the flip is OFF by default for
   it. Keep the flip as a configurable knob (`BakeOptions::yflip_normal`)
   for F23 user-imports. The doc's `splatDetailNormalDiffuseAlpha = 0`
   baseline guidance is still correct.
8. `/home/teague/code/BARMapEditor/docs/research/textures/claude-findings-from-research.md`
   — section on the `splatDetailNormalTex requires specularTex` silent
   disable. **The source-audit at
   `docs/research/source-audit-2026-05-18/FINDINGS.md` §7.2 partially
   refutes this** — Recoil's current C++ render-state gates DNTS on
   `splatDistrTex && splatDetailNormalTex[]`, NOT specularTex. Without
   spec the result still looks visually flat, so keep the lint warning,
   but reword it (see FINDINGS for the new wording).
9. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`** §7
   — the ground-truth fragment-shader composite math from
   `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`.
   D2 only consumes the texture-layout half (`splatDetailNormalTex` as
   `(R=nx, A=nz)` for the BASE map normal vs the standard `(R,G,B) =
   (nx, ny, nz)` for the DNTS layers), but D3 / D4's downstream work
   inherits the rest. Note that the base normal in §7.5 is NOT the
   same encoding as the DNTS layers — DNTS textures use full RGB
   normal (decoded `* 2 - 1`), the base SMF normal uses R+A with
   Y derived.
10. **Direct source: `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`**
    lines 87-95 (DNTS uniforms) and 174-199 (composite). The Y-flip
    decision in D2 is justified by the `* 2 - 1` decode in line 183 —
    if the green channel encoding doesn't match OpenGL tangent space,
    every DNTS layer's lighting will be wrong.
9. ADR-014 (Compressonator vendored at `tools/compressonator/`), ADR-012
   (PyMapConv subprocess driver — reference pattern for invoking
   external CLIs from the pipeline crate), ADR-018 (Brush trait — D3's
   SplatBrush mirrors this exactly), ADR-025 / ADR-027 (texture pack —
   D2 consumes these slots).
10. `crates/barme-pipeline/src/lib.rs` and `crates/barme-pipeline/Cargo.toml`
    — D2 lands here.
11. `crates/barme-core/src/brushes/mod.rs` — D3 mirrors this trait shape.
12. `tools/textures/` — D2's inputs. Must exist (D1 fetched them); if
    not, run `./scripts/fetch-textures.sh` before proceeding.
13. `tools/compressonator/CompressonatorCLI` — D2's compressor. Verify
    binary present.

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new stage-1-dnts-bake
./devlog/log.sh new stage-1-splat-module
```

Fill each from phase-3-plan.md.

## Step 3 — Scope

In order, one commit per item:

1. **D2 — DNTS bake pipeline [ADR-026]**
   - New module `crates/barme-pipeline/src/dnts.rs`.
   - Public API:
     ```rust
     pub struct BakeOptions {
         pub diffuse_in_alpha: bool,  // false = ship the safer default
     }
     pub fn bake_dnts(
         slot_dir: &Path,
         out_dds: &Path,
         opts: BakeOptions,
     ) -> Result<()>;
     ```
   - Bake pipeline:
     1. Read `diffuse.{png,jpg}` and `normal.png` from `slot_dir` via the
        `image` crate (already a workspace dep). The D1-shipped starter
        pack always provides PNG diffuse; the JPG fallback is for future
        F23 user-imported assets.
     2. **Y-flip the normal map green channel** — configurable, default
        OFF for the starter pack. Recoil uses OpenGL tangent-space
        normals (per source-audit FINDINGS §7.4:
        `SMFFragProg.glsl:276-278` builds the TBN with the standard
        +Y-up basis). The D1-shipped starter pack extracts
        `*_NormalGL.png` from ambientCG, which is **already OpenGL
        convention** — no Y-flip required. The flip stays as a
        configurable `BakeOptions { yflip_normal: bool }` knob for
        F23 user-imports where the source is DirectX-convention
        (e.g. Substance / Quixel exports). Write a dedicated unit
        test against a known-direction synthetic normal map that
        asserts `yflip_normal: true` inverts the green channel
        (`255 - g`) and `yflip_normal: false` is a passthrough.
     3. Compose to a `splatDetailNormalTex`-format image:
        - RGB ← normal (Y-flipped iff `opts.yflip_normal == true`;
          OFF for the ambientCG starter pack).
        - A ← `0xFF` if `opts.diffuse_in_alpha == false` (ADR-025
          baseline); otherwise high-pass-filtered diffuse. **Ship with
          `diffuse_in_alpha: false` this sprint; the high-pass path is
          ADR-034 (deferred).**
     4. Compress to BC3 (DXT5). Invoke `tools/compressonator/CompressonatorCLI`
        via `std::process::Command`, mirroring the pattern in
        `crates/barme-pipeline/src/pymapconv.rs`. Capture stdout/stderr;
        pipe to `tracing::trace!`.
     5. Emit `<slot>_dnts.dds` to `out_dds`.
   - **Cache**: sha256 of `(diffuse_bytes, normal_bytes, opts)` →
     `tools/textures-cache/<sha>.dds`. If the cache hit exists, copy
     instead of re-baking. Cache is gitignored.
   - Writes ADR-026.

2. **D3 — `barme-core::splat` module + dirty-rect upload**
   - New module `crates/barme-core/src/splat.rs`:
     ```rust
     pub struct SplatDistribution {
         pub width: u32,
         pub height: u32,
         pub rgba: Vec<[u8; 4]>,
     }
     pub struct SplatStamp {
         pub world_x: f32,
         pub world_z: f32,
         pub radius: f32,
         pub strength: f32,  // 0..=1
         pub channel: SplatChannel,
     }
     pub enum SplatChannel { R, G, B, A }
     pub trait SplatBrush: Send + Sync + 'static {
         fn id(&self) -> &'static str;
         fn label(&self) -> &'static str;
         fn apply(
             &self,
             dist: &mut SplatDistribution,
             stamp: SplatStamp,
         ) -> Option<DirtyRect>;
     }
     pub struct SplatBrushRegistry { /* mirrors BrushRegistry */ }
     ```
   - **Dims convention: ship a fixed 1024 × 1024 RGBA distribution
     (4 MB) regardless of map size.** This is the recommended default
     after auditing real BAR maps:
     `scratch/bar-maps/extracted/titanduel/maps/titandueldist.png` is
     1024 × 1024;
     `scratch/bar-maps/extracted/comet/maps/splat_distr.png` is
     2048 × 1024 (non-square, ~half the resolution per axis vs the
     SMT 8192² diffuse). The engine accepts ANY dimension — see the
     "resolution-flexible" note below.
   - **Important math finding from the audit** (verified against
     `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
     line 177 + `rts/Map/SMF/SMFReadMap.cpp` line 281–282 at HEAD
     2026-05-18): the distribution texture is sampled at
     `texture2D(splatDistrTex, uv)` with `uv` in `[0,1]^2` spanning
     the whole map — same UV space as the base SMT diffuse. The
     engine reads `splatDistrTexBM.xsize/ysize` from whatever the
     `.png`/`.tga` happens to be and uses those values; there is no
     fixed convention. The per-channel weight is then multiplied by
     `splats.texMults` from mapinfo.
   - **Sizing rationale**: 1024² × 4 = 4 MB CPU buffer, a comfortable
     middle ground vs the heightmap (2 MB at 16 SMU). Painting at this
     resolution gives ~64 elmos / pixel on a 16-SMU map (the SMT
     diffuse is 8 elmos / pixel for reference; metalmap is 16
     elmos / pixel). Larger map → coarser per-pixel coverage; rely on
     `splats.texScales` to drive visible detail tile size, NOT the
     distribution resolution. **Document this as ADR-027 if the
     fixed-1024² choice isn't already captured there from D1.**
   - Sanity-check via a paint-and-export smoke test — paint a single
     green stamp in the editor at world (4096, 4096) on a 16-SMU map,
     build the `.sd7`, load it in BAR, confirm the green DNTS slot
     blends where you painted.
   - Three initial brushes (struct-per-brush, registry pattern from
     ADR-018):
     - `PaintChannel` — writes 255 to the stamp's channel with a
       smoothstep falloff. Other channels clamped down to keep
       `R + G + B + A <= 255` per BAR's normalisation rule.
     - `Erase` — sets the stamp's channel toward 0 with smoothstep falloff.
     - `Smooth` — 3×3 average per pixel, then lerp toward the average by
       `strength * falloff`.
   - Dirty-rect pattern matches ADR-018: brush computes the pixel bbox of
     the stamp, walks only those pixels, returns the bbox.
   - **Unit tests**:
     - Painting a 100-elmo G stamp on an empty distribution → centre
       pixel G=255, R=B=A=0.
     - Erase reverses paint within tolerance.
     - Smooth reduces local variance.
     - Channel sum invariant: after PaintChannel, no pixel sums to >255
       across RGBA.
   - **No GPU code** — that's D4 (Sprint 9).
   - **No UI code** — that's D5 (Sprint 9).
   - **No undo integration** — splat distribution is too large for the
     existing undo channel (a 4096² distribution is 64 MB — single stroke
     could blow the cap). Defer splat-undo to a follow-up that adapts
     A1's bitset pattern. Document in phase-3-plan.md and a TODO in the
     module.
   - No ADR (mirrors ADR-018's shape; no new architectural decision).

Then a **3rd rollup commit**: STATUS UPDATEs in SRS / ROADMAP, tick 2
boxes in phase-3-plan.md, closing devlog log.

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells.
- Before every commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects.
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing convention.
- Devlog folder per item.

## Step 5 — Out of scope

- D4 / D5 / D6 / D7 — splat shader, UI, emission, minimap. Later sprints.
- Splat undo channel — explicit deferral; TODO note only.
- The mapinfo `resources.splatDetailNormalTex` field population — D6 wires
  that.
- Validating `splatDetailNormalTex` requires `specularTex` — the lint pass
  (C8 / Sprint 13) enforces.
- GPU compute splat brushes — defer like ADR-021 deferred GPU heightmap
  brushes. CPU first.

## Step 6 — Critical pitfalls (read twice)

From phase-3-plan.md D2 / D3 + research digests + SRS §2.1:

1. **Y-flip is silent-failure if misapplied** (either direction).
   The D1-shipped starter pack ships ambientCG `*_NormalGL.png` which
   is **already OpenGL convention** — flip OFF is the correct call,
   per source-audit FINDINGS §7.4. Flipping ON for a GL source
   produces inverted concavity (lighting upside-down on a slope);
   leaving OFF for a DX source produces the same artifact in the
   other direction. Default the `BakeOptions::yflip_normal` to
   `false` for the starter pack; expose a per-import override at
   F23. Both branches need unit-test coverage against a
   known-direction synthetic normal.

2. **JPEG normal maps**: D1 enforces PNG; D2 must defensively re-check.
   If `normal.png` doesn't exist or `image::open` reports a non-PNG
   format, error out with a clear message. Don't silently fall back.

3. **Compressonator invocation pattern**: capture stdout AND stderr;
   stream to `tracing::trace!`. Don't silently ignore stderr —
   Compressonator emits useful warnings (e.g. "non-power-of-2 dimensions")
   that bubble up if texture sizes are wrong.

4. **Cache invalidation**: the sha256 must include `BakeOptions` in its
   input. Otherwise toggling `diffuse_in_alpha` won't invalidate the
   cache.

5. **Splat distribution dimension is NOT alignment-sensitive**: the
   engine samples `splatDistrTex` at `uv ∈ [0,1]²` covering the whole
   map (`SMFFragProg.glsl:177`), so 1024² is fine regardless of
   `smu_x × smu_z`. The risk is visual ("brush feels chunky on a
   32-SMU map") not correctness. If a smoke test shows alignment
   drift, the bug is in `world_to_uv`, NOT in the buffer size.

6. **Channel sum invariant**: BAR's renderer caps the normal-blend
   strength at `min(1.0, dot(splatCofac, vec4(1.0)))` (per
   `SMFFragProg.glsl:180`, source-audit FINDINGS §7.3). So overweighted
   channels don't physically over-brighten the normal-blend, but they
   DO over-bias the diffuse offset (which only clamps at the end).
   Recommendation: keep `R + G + B + A <= 255` as the editor's
   normalization rule for user predictability. When `PaintChannel`
   writes 255 to G, the other 3 channels must clamp down proportionally.
   Test asserts the invariant.

7. **Distribution memory at the 1024² fixed dim**: 4 MB resident. Still
   too large to copy-snapshot per stroke (would evict 25-ish heightmap
   strokes from the 100 MB undo cap) — defer splat-undo per "No undo
   integration" scope note. The dirty-rect upload pattern from
   ADR-018 still applies for *shader* updates (D4), not undo.

8. **Brush trait `Send + Sync + 'static`**: matches the existing `Brush`
   trait in ADR-018. Don't drop the bound — the registry walks `Box<dyn
   SplatBrush>` from multiple call sites (UI, future tests).

9. **`splatDetailNormalDiffuseAlpha = 0` baseline** per ADR-025. The
   alpha channel is `0xFF` solid in this sprint's DDS output. The
   `diffuse_in_alpha: true` branch is implemented but untested in BAR
   until ADR-034 lands.

10. **Compressonator's BC3 output**: BC3 carries 8-bit alpha. BC1 (which
    we'd use if `diffuse_in_alpha == false` strictly) would save space
    but locks us out of the future high-pass workflow. Pick BC3 always —
    keeps the upgrade path open. If per-slot DDS exceeds 1 MB at 1024²
    inputs, reopen.

## Step 7 — Exit criteria

- 3 commits on `main`: D2, D3 + rollup.
- 2 devlog folders filled.
- 2 checkboxes ticked in phase-3-plan.md.
- ADR-026 in `docs/DECISIONS.md`.
- SRS / ROADMAP STATUS UPDATEs (DNTS pipeline shipped, splat module
  shipped, both gated on D4/D5 for visible feature).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - `bake_dnts(grass_meadow_slot, "out.dds", BakeOptions::default())`
    produces a valid BC3 DDS (inspect with a DDS viewer or `dds-info`
    CLI — file header reports DXT5 / BC3).
  - Y-flip unit tests pass: a deterministic synthetic normal map fed
    through `yflip_normal: true` shows the green channel inverted
    (`255 - g`); the same input through `yflip_normal: false` is a
    passthrough. Starter-pack default is `false`.
  - Re-baking with identical inputs is a no-op (cache hit; log shows
    `dnts: cache hit slot=grass-meadow`).
  - Toggling `diffuse_in_alpha` invalidates the cache.
  - `SplatDistribution::new(map_size)` allocates correct dimensions
    (verify dimension matches the BAR canonical source — record the
    answer in the devlog log).
  - PaintChannel G stamp at 100-elmo radius → centre pixel G=255, others
    clamped per invariant.
  - All splat unit tests green.
- Final devlog log summarising what shipped + "Sprint 9 = D4 + D5 (splat
  fragment shader + splat tool UI)" handoff note. Capture the
  distribution-dimension answer for the next session.

Start by running `git status` and reading the files in Step 1. Verify
`tools/textures/` is populated before writing D2 code (if not, run the
fetch script from D1). Begin with D2 — D3's tests don't need DDS output,
but having D2 working first lets you eyeball-check the bake pipeline
against your splat painting later.
