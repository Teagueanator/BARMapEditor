# Sprint 8 — DNTS bake pipeline + splat module (D2, D3)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 8** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship **D2 + D3** — the DNTS bake pipeline (Y-flip +
Compressonator + DDS emit) and the `barme-core::splat` module
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
   — sections on Y-flip (Recoil's OpenGL tangent space requires inverted
   green channel — primary-source evidence: `_flipped` suffix in shipped
   BAR DNTS files) and the `splatDetailNormalDiffuseAlpha = 0` baseline.
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
     1. Read `diffuse.{jpg,png}` and `normal.png` from `slot_dir` via the
        `image` crate (already a workspace dep).
     2. **Y-flip the normal map green channel** — non-negotiable. Recoil
        uses OpenGL tangent-space normals; ambientCG ships DirectX-
        convention. Write a dedicated unit test that loads a known-
        direction normal map, runs the flip, and asserts each pixel's
        green channel inverted (`255 - g`).
     3. Compose to a `splatDetailNormalTex`-format image:
        - RGB ← Y-flipped normal.
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
   - Dims convention: distribution is `(128 * smu_x, 128 * smu_z)` —
     **NOT** 512 px/SMU. Verify against `crates/barme-pipeline/src/mapinfo.rs`
     and the SMF format docs in SRS §1.2 before locking. The BAR splat
     distribution texture is `4096²` at 16-SMU per ZK reference; that's
     256 px/SMU, halfway between metalmap (32 px/SMU) and SMF tile pool
     (512 px/SMU). **Find the canonical dimension before sizing the
     buffer**; don't assume.
   - **Important math finding from the audit:** in the engine shader
     (`SMFFragProg.glsl:177`), the distribution texture is sampled at
     `texture2D(splatDistrTex, uv)` with `uv` in `[0,1]^2` spanning
     the whole map — same UV space as the base SMT diffuse. The
     per-channel weight is then multiplied by `splats.texMults` from
     mapinfo. This means the distribution texture itself is
     resolution-flexible: 4096² is BAR convention, but the shader
     never assumes it. Pick a dim that fits the editor's brush
     resolution; 256 px/SMU is reasonable. Sanity-check via a
     paint-and-export smoke test — paint a single green stamp in the
     editor at world (4096, 4096) on a 16-SMU map, build the `.sd7`,
     load it in BAR, confirm the green DNTS slot blends where you
     painted.
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

1. **Y-flip is mandatory and silent-failure.** Skipping it produces
   inverted concavity under lighting — subtle enough to ship and embarrass
   us. The dedicated unit test is the single most important deliverable
   in D2.

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

5. **Splat distribution dimension**: **verify against canonical BAR
   source before allocating**. Wrong dimension means the splat map is
   misaligned with the metalmap / typemap / heightmap. If unsure, write
   a 4-SMU smoke test: paint G on the centre 100-elmo radius, dump to
   PNG, eyeball alignment against the heightmap centre.

6. **Channel sum invariant**: BAR's renderer caps the normal-blend
   strength at `min(1.0, dot(splatCofac, vec4(1.0)))` (per
   `SMFFragProg.glsl:180`, source-audit FINDINGS §7.3). So overweighted
   channels don't physically over-brighten the normal-blend, but they
   DO over-bias the diffuse offset (which only clamps at the end).
   Recommendation: keep `R + G + B + A <= 255` as the editor's
   normalization rule for user predictability. When `PaintChannel`
   writes 255 to G, the other 3 channels must clamp down proportionally.
   Test asserts the invariant.

7. **Distribution memory at 16 SMU**: at 256 px/SMU (verify) that's
   4096² × 4 bytes = 64 MB. **DO NOT** copy-snapshot for undo — see the
   "No undo integration" scope note. The dirty-rect upload pattern from
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
  - Y-flip unit test passes (deterministic input → expected green channel
    inversion).
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
