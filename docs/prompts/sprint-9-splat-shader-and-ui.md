# Sprint 9 — Splat fragment shader + splat tool UI (D4, D5)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 9** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship **D4 + D5** — the GPU-side splat texture + fragment
shader blend (D4) and the splat tool UI (D5). After this sprint, the user
can paint DNTS layers in the editor and see live preview, but **the
painted distribution does NOT yet flow into the `.sd7`** — D6 wires that
in Sprint 11.

**Prerequisites:** Sprint 8 (D2 + D3) MUST be ticked. D2 owns the DNTS bake
pipeline; D3 owns `SplatDistribution` + `SplatBrush`. D4 reuses both.

**Sequencing note:** Sprint 10 (mapinfo audit fix) can run before OR
after Sprint 9. If Sprint 10 lands first, the `validation_summary`
extension in D5 inherits the corrected schema (esp. the
`splatDetailNormalTex` subtable form + the `sundir`/`sunDir` dual
emit). If Sprint 9 lands first, the `validation_summary` chip wired
in D5 will surface against the OLD schema and Sprint 10 will need to
re-touch one call site. Sprint 9 itself does NOT touch the mapinfo
emitter — D6 wires emission in Sprint 12.

This sprint is **performance-sensitive**: the splat distribution is 1–4 MB
of `rgba8unorm` data that has to upload to GPU on every brush stroke.
Dirty-rect uploads (from D3) are mandatory; full-texture uploads will
miss the 8 ms brush-stroke NFR.

**UX context (ADR-035 — UI overhaul, already shipped):** D5 is **not a
greenfield Inspector write**. The editor already has:

- `Tool::SplatPaint` variant in the `Tool` enum (keyboard `T`,
  `Icon::Splat`, label "Splat paint"). The left tool strip already
  shows it; no enum change needed.
- A scaffolding Inspector at
  `crates/barme-app/src/main.rs::inspector_splat` that renders a
  4-row RGBA layer list, RGBA channel chips, brush-mode buttons
  (Paint / Erase / Smear), and three ramp sliders (radius / strength /
  spacing). State lives on `App::splat_state: SplatState` with seeded
  Grass / Rock / Sand / Snow layers.
- A reusable widget library at `crates/barme-app/src/ui/widgets.rs`
  exposing `section(ui, title, accent, right_fn, body_fn)`,
  `chip(ui, tone, label)`, `ramp_slider_labelled(...)`,
  `pill_toggle(...)`, `split_button(...)`, `key_combo(...)`,
  `icon_button(...)`.
- A line-icon set at `crates/barme-app/src/ui/icons.rs` painted via
  `egui::Painter` (no font dep). `Icon::Splat`, `Icon::Brush`,
  `Icon::Spray`, `Icon::Layers`, `Icon::Plus`, `Icon::X`, etc.
- A dark palette at `crates/barme-app/src/ui/theme.rs::Tokens::DARK`
  (bg / panel / panel2 / hover / border / border_hi / text / muted /
  dim / accent / accent_dim / chip tones). All colours come from here.
- A global symmetry cluster in the top action bar. Whatever `Tool`
  is active, the active `SymmetryAxis` is read from `App::symmetry`.
  **D5 does not add per-tool symmetry UI.** The existing
  `ui::overlay::paint_symmetry_overlay` + the symmetry-replication
  call in the central viewport already work for any tool that
  consumes pointer input on the heightmap rect.
- A mini-map at `crates/barme-app/src/ui/minimap.rs` in the
  viewport's top-right corner. **There is no XYZ nav gizmo** —
  ADR-035 retired it. References to `gizmo_rect` /
  `nav_gizmo_drag_active` are vestigial; the central viewport's
  click-suppression now reads `minimap_rect` instead.

**What D5 is doing**, then, is: replace the in-memory `SplatState`
scaffolding with a persisted `Project.splat_config` driven by your
D3 brushes, wire pointer input from the central viewport into
`SplatBrush::apply`, and render slot thumbnails from D1's
`tools/textures/<slot>/diffuse.*`. Visual layout follows the
already-shipped mockup; you're rewiring the data layer beneath it.

**ADR numbering:** ADR-035 is **already taken** by the UI overhaul. The
splat fragment shader ADR introduced in D4 below should be assigned
the next free ADR number — check `docs/DECISIONS.md` (likely
**ADR-036**) and use that throughout.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (DNTS / splat schema),
   §2.1 #11 ("3D preview ≠ in-game rendering"), §3.2 F4, §3.3 NFRs
   (8 ms brush stroke budget).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable
   rules. **Pitfalls 16, 17 (added 2026-05-18 by source audit) are
   D4 inputs:** SMT base normal encoding (R + A channels), specular
   exponent formula.
4. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`**
   §7 — the corrected fragment-shader composite math, sourced directly
   from `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`.
   **This supersedes the proposed-ADR WGSL** in
   `docs/research/splat-rendering/claude findings.md` (drafted before
   the ADR was renumbered) where the two disagree. Five corrections
   to internalize:
   - **§7.1** Constant is `SMF_INTENSITY_MULT` (with T), not
     `SMF_INTENSITY_MUL`.
   - **§7.2** DNTS gating is `splatDistrTex && splatDetailNormalTex[]`,
     NOT specularTex-dependent. Reword the lint warning.
   - **§7.3** Composite math: per-channel UV multipliers come from
     `splatTexScales.{r,g,b,a}`; each layer is decoded `* 2 - 1`
     across the FULL RGBA (alpha too); `splatCofac = dist * texMults`
     is applied to the whole vec4; normal-blend strength is
     `min(1.0, dot(splatCofac, vec4(1.0)))`.
   - **§7.4** Tangent basis is per-fragment from `normalsTex`:
     `tTangent = normalize(cross(normal, vec3(-1,0,0)))`. NOT static
     `T=+X / B=+Z`.
   - **§7.5** Base normal decoded from `normalsTex.ra` (X and Z),
     Y derived. NOT a generic RGB normal sample.
   - **§7.6** Specular exponent is `specularCol.a * 16.0` (flat),
     NOT a `mix(16, specularExponent, alpha)`.
5. `/home/teague/code/BARMapEditor/docs/research/splat-rendering/claude findings.md`
   — the draft ADR that will become ADR-036. Read it FIRST for the
   shape (bind group layout, uniform struct, etc.) — then apply the
   §7 corrections from FINDINGS as you translate to WGSL. Note: this
   research file was drafted as "ADR-035"; UI-overhaul work took that
   number first, so the splat-shader ADR is renumbered to ADR-036.
6. `/home/teague/code/BARMapEditor/docs/research/splat-rendering/Gemini Terrain Shader Composite Research.md`
   — Gemini's parallel pass. Cross-check.
7. **Direct source references:**
   - `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
     — the canonical fragment shader. Lines 174-199 are the DNTS
     composite. **Diff your WGSL against this before merging.**
   - `/home/teague/code/RecoilEngine/rts/Map/SMF/SMFRenderState.cpp:114`
     — the gating logic. Confirms specularTex is no longer in the
     DNTS gate.
8. `/home/teague/code/BARMapEditor/devlog/README.md`.
9. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` —
   read **D4 and D5 in full**, plus skim D6 to understand what your
   data shapes must support.
10. ADR-018 (Brush trait — D3's `SplatBrush` uses this; D4 doesn't
    change it), ADR-026 (DNTS bake, from D2), ADR-017 (heightmap GPU
    upload pattern — D4 mirrors).
11. `crates/barme-app/src/terrain.wgsl` — the existing fragment shader
    you'll extend.
12. `crates/barme-app/src/render.rs` — bind group definitions; D4
    adds bindings.
13. `crates/barme-core/src/splat.rs` (from D3) — the
    `SplatDistribution` that D4 uploads.

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new stage-1-splat-shader
./devlog/log.sh new stage-1-splat-tool-ui
```

Fill each from phase-3-plan.md.

## Step 3 — Scope

In order, one commit per item:

1. **D4 — GPU side: splat texture + fragment shader blend (ADR-036)**
   - Bind group extension (`crates/barme-app/src/render.rs`):
     - Add `splat_distr_tex: wgpu::Texture` (`rgba8unorm`, size =
       distribution dims from D3).
     - Add `dnts_tex_array: wgpu::Texture` (`rgba8unorm`, layered,
       4 layers — one per active slot). **Use a texture array, not 4
       independent textures**, to minimize bind-group churn on slot
       reassignment.
     - Add `splat_uniforms` storage buffer with:
       ```wgsl
       struct SplatU {
         tex_scales: vec4<f32>,   // splats.texScales
         tex_mults:  vec4<f32>,   // splats.texMults
         diffuse_in_alpha: u32,   // splatDetailNormalDiffuseAlpha flag
       };
       ```
   - Fragment shader (`crates/barme-app/src/terrain.wgsl`):
     - Translate the DNTS composite from `SMFFragProg.glsl:174-199`
       per FINDINGS §7.3, with the corrections in §7.4-§7.6.
     - **DO NOT** use the WGSL from
       `docs/research/splat-rendering/claude findings.md` verbatim —
       it has the static-tangent / RGB-normal / mix-exponent bugs.
     - Editor-preview simplifications stay: no shadow sampling, no
       water absorption, no skyReflectModTex.
     - Lint rule: warn (don't gate) when `splatDetailNormalTex[]` is
       set but `specularTex` is absent. Wording from FINDINGS §7.2.
   - Upload path (`write_splat_rect`):
     - Mirror `write_heightmap_rect` (ADR-017): pixel rect in
       distribution coordinates, sub-uploaded via
       `queue.write_texture`.
     - Pixel rect from the `DirtyRect` returned by `SplatBrush::apply`.
   - **Verify**: paint G across half the map, slot-1 (grass) bound to
     G → grass diffuse visible on painted half, slot-0 elsewhere.
     This is the success criterion from phase-3-plan.md D4.
   - Writes ADR-036 with the corrected WGSL inline. (The number-after
     -overhaul shift — ADR-035 became the UI overhaul; the splat
     shader claims the next number.)

2. **D5 — Splat tool UI**
   - **Use the existing `Tool::SplatPaint` variant** — ADR-035
     already shipped it. Do **not** add `Tool::Splat`; rename any
     remaining references in your plan / commit messages to
     `SplatPaint`.
   - **Replace `App::splat_state` (Phase-7 scaffolding) with persisted
     state on `Project`**:
     ```rust
     // barme-core/src/project.rs
     #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
     pub struct SplatConfig {
         /// Indices into the texture registry (D1 / D3 slot ids), one
         /// per RGBA channel. `None` = channel unbound.
         pub channels: [Option<u8>; 4],
         /// `splats.texScales` — per-channel detail UV multiplier.
         pub tex_scales: [f32; 4],
         /// `splats.texMults` — per-channel weight scale.
         pub tex_mults:  [f32; 4],
         /// `splats.splatDetailNormalDiffuseAlpha` (ADR-025 baseline = false).
         pub diffuse_in_alpha: bool,
     }

     // Project struct gets:
     #[serde(default)]
     pub splat_config: SplatConfig,
     ```
     Delete `App::splat_state` and its initializer. Add
     `self.dirty = true;` everywhere the new `splat_config` mutates.
   - **Inspector rewrite** — keep the visual layout from
     `inspector_splat`'s Phase-7 scaffolding; swap its backing data:
     - **TEXTURE LAYERS section** (uses
       `widgets::section(ui, "Texture layers", true, |ui| chip(RGBA), |ui| {...})`):
       Four rows, one per RGBA channel. Each row contains an
       active-channel radio, a coloured channel chip (R / G / B / A),
       a slot swatch loaded from `tools/textures/<NN-slot>/diffuse.{jpg,png}`
       at small (~22 px) size into an `egui::TextureHandle` cached
       per slot on the `App`, the slot name (from `meta.toml`), and
       an opacity bar driven by `tex_mults[i].clamp(0.0, 1.0)`.
       Clicking the slot swatch opens a popover with a
       **slot picker grid** (3×N like the Geo features panel),
       walking `tools/textures/` and rendering each slot's diffuse
       thumbnail; click assigns that slot to the row's channel.
     - **BRUSH section** (`widgets::section(ui, "Brush", false, ...)`):
       Three buttons (Paint / Erase / Smear) backed by the D3
       brush registry (`SplatBrushRegistry::get("paint")` etc.).
       Three `ramp_slider_labelled` rows for radius, strength,
       spacing — driven by new fields on a small `SplatBrushState`
       struct on `App` (radius / strength / spacing are session-
       only, not project-persisted, matching the heightmap brush).
     - **PER-LAYER TUNING section** (new):
       Two `ramp_slider_labelled` rows for the *currently-selected*
       channel's `tex_scale` (range `0.0015..=0.05`, default `0.02`,
       tooltip *"BAR convention. Real maps use 0.0015–0.008"*) and
       `tex_mult` (range `0.0..=4.0`, default `1.0`). Edits flow into
       `Project.splat_config` and mark dirty.
     - **GLOBAL section** (new): a `pill_toggle` for
       `diffuse_in_alpha` (label "Diffuse in α", tooltip:
       *"Splat-detail-normal alpha channel carries a high-pass
        diffuse offset. Baseline off — ADR-034 enables once stable."*).
   - **Symmetry**: the global symmetry cluster in the top bar already
     drives `App::symmetry`. The central-viewport pointer dispatch
     calls `SymmetryAxis::replicate` to produce N stamps per click.
     **D5 does not add a per-tool symmetry control.** Wire your
     `SplatBrush::apply` into the same call site that
     `place_start_position` and `apply_brush_at` use; the dirty-rect
     union from D3 fans out to the GPU upload path from D4.
   - **Canvas pointer dispatch**: extend the existing match in
     `central()` to handle `Tool::SplatPaint`: LMB drag/stamp → call
     into D3's brush registry with the selected channel. RMB =
     orbit, same as every other tool. **No splat-erase on RMB** —
     erase is the "Erase" brush button.
   - **Mini-map**: in `crate::ui::minimap::paint_minimap`, add an
     optional `splat_distribution: Option<&SplatDistribution>` arg
     and render the distribution body as a translucent overlay on
     top of the heightfield thumbnail when supplied (50 % opacity,
     RGB channels → red / green / blue, alpha channel desaturated).
     Wire it from `central()` so the user can see at-a-glance where
     they've painted.
   - **Validation chip**: extend
     `App::validation_summary` to surface the
     `splatDetailNormalTex` without `specularTex` lint (warn-tone)
     when any `Project.splat_config.channels[i].is_some()` but no
     specular texture is set. The chip already renders in the top
     bar.
   - No new ADR — the existing ADR-035 (UI overhaul) covers the
     widget contract; the D4 shader ADR (ADR-036) covers the GPU
     side.

Then a **3rd rollup commit**: STATUS UPDATEs in SRS / ROADMAP, tick 2
boxes in phase-3-plan.md, closing devlog log.

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells.
- Before every commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects.
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing convention: `info!` on slot assignment + brush activation;
  `trace!` on per-stamp diagnostics; `warn!` on no-specularTex lint.
- Devlog folder per item.

## Step 5 — Out of scope

- D6 — mapinfo emission of the painted distribution + `.sd7` bundling.
  Without D6, painting works in the editor but does NOT round-trip into
  a playable map. Document loudly.
- Splat undo channel — same explicit deferral from Sprint 8. The
  distribution is too large for the existing UndoEntry cap.
- Specular texture painting — covered by a future item once F4 splats
  ship.
- Real-time shadows / `groundShadowDensity` — preview stays at
  shadow = 1.0 (FINDINGS §7 editor-preview deferrals).
- Per-fragment exponent from `specularTex.a` if the editor doesn't yet
  load a specular texture — fall back to a constant 16.
- Atmospheric scattering / tone mapping / fog blend — explicit non-goal
  per FINDINGS §7 caveats.

## Step 6 — Critical pitfalls (read twice)

1. **DO NOT copy the proposed WGSL from
   `docs/research/splat-rendering/claude findings.md` verbatim** —
   it has five load-bearing bugs (FINDINGS §7.1-§7.6). Use the engine
   GLSL at `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
   as the source of truth.

2. **Tangent basis is per-fragment from `normalsTex`**, not a static
   `T = +X / B = +Z` basis. The static basis works on flat ground but
   visibly skews on slopes. Engine code (`SMFFragProg.glsl:276-278`):
   ```glsl
   vec3 tTangent = normalize(cross(normal, vec3(-1.0, 0.0, 0.0)));
   vec3 sTangent = cross(normal, tTangent);
   ```

3. **Base normal encoding: R + A channels, Y derived**. The engine
   reconstructs the world-up component from `sqrt(1 - dot(xz, xz))`
   (`SMFFragProg.glsl:146-150`). If the editor's terrain mesh derives
   its normal from a separate path (CPU-computed per-vertex from the
   heightmap), be sure that the encoding sent to the fragment shader
   matches.

4. **DNTS textures are DIFFERENT** — they decode standard `(R, G, B) =
   (nx, ny, nz)` via `* 2 - 1`. Don't confuse the two encodings.

5. **Specular exponent is `specularCol.a × 16.0`**, NOT a `mix` of the
   global `specularExponent` uniform and 16. The Lua
   `lighting.specularExponent` is only used when NO specular texture
   is bound (`SMFFragProg.glsl:414-416`).

6. **`SMF_INTENSITY_MULT`** (with T). Macro is in the engine at
   shader line 4. The constant pre-dims ambient + diffuse contributions.
   Apply this CPU-side (multiply the uniform before upload) — it's
   simpler and the lint rule from FINDINGS §7 catches drift.

7. **Texture binding count budget**: heightmap (1) + base normal (1)
   + splat distribution (1) + 4×DNTS via texture-array (1, but with
   `array_size = 4`) + specular (1) = 5 bindings. Well within wgpu's
   16-binding default.

8. **Texture array vs 4 textures**: prefer the array. Slot reassignment
   becomes `queue.write_texture(..., layer = N, ...)` instead of
   recreating the bind group. Saves a frame's stall on slot change.

9. **Dirty-rect upload only**: full-texture writes at 4096² × 4 bytes
   = 64 MB per stroke blow the 8 ms NFR. Use the `DirtyRect` returned
   by `SplatBrush::apply` and call `queue.write_texture` with the
   sub-rect origin + extent.

10. **Symmetry replication**: when symmetry is on, the painter
    produces N derived stamps per click. Their dirty rects union
    into a single upload (ADR-019 pattern, already implemented for
    heightmap). Apply the same union here.

11. **No splat-erase on RMB.** RMB is orbit. Erasing is a
    `Tool::SplatPaint` sub-mode (the "Erase" brush button in the
    Inspector's BRUSH section, backed by D3's erase brush).
    Toggle via Inspector or hold-Shift.

12. **A4 lint rule**: when the user enables a DNTS slot but no specular
    texture is loaded, surface the warning per FINDINGS §7.2 wording.
    Don't gate — Recoil's current renderer no longer requires
    specularTex to enable the DNTS branch.

## Step 7 — Exit criteria

- 3 commits on `main`: D4, D5 + rollup.
- 2 devlog folders filled.
- 2 checkboxes ticked in phase-3-plan.md.
- ADR-036 in `docs/DECISIONS.md` (with the source-audited corrected
  WGSL inline; reference FINDINGS §7). The UI overhaul holds ADR-035.
- SRS / ROADMAP STATUS UPDATEs (splat shader shipped, splat tool UI
  shipped, gated on D6 for `.sd7` round-trip).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Editor launches; **Splat paint** tile in the left tool strip
    (keyboard `T`) is selectable. The tile shows the `Icon::Splat`
    glyph + "T" letter and lights with the accent rail when active.
  - Inspector swaps to the rebuilt Splat panel: TEXTURE LAYERS
    section (4-row RGBA list with thumbnails populated from
    `tools/textures/<slot>/diffuse.*`), BRUSH section (Paint /
    Erase / Smear chip-buttons + radius / strength / spacing
    `ramp_slider_labelled` rows), PER-LAYER TUNING section
    (`tex_scale` and `tex_mult` ramp sliders), GLOBAL section
    (`diffuse_in_alpha` pill toggle).
  - Click a layer row's slot thumbnail → slot picker grid pops up;
    click a slot → that channel rebinds; thumbnail updates.
  - Paint a stroke → preview visibly shows the bound slot's diffuse
    where painted.
  - Enable Horizontal symmetry from the **top-bar symmetry cluster**
    (NOT a per-tool control). Paint a stroke → mirrored stroke
    appears on the other side within one frame.
  - The mini-map (top-right of the viewport) reflects the painted
    region as a translucent RGBA overlay on its heightfield
    thumbnail.
  - Validation chip in the top action bar flips to a warn-tone "no
    specularTex" message when a DNTS slot is bound and the project
    has no specular texture — confirming the
    `validation_summary` extension.
  - Re-bind slot 5 (sand) to channel G mid-session → preview updates
    without recreating the bind group (texture-array layer write);
    Inspector swatch updates within the same frame.
  - Save the project (Save button in the top bar; dirty dot clears).
    Reopen — the `SplatConfig` round-trips, the four channels show
    the same slot bindings, scales, and mults.
  - `cargo test --workspace -- splat` runs all splat tests green
    (D3 tests still pass + new D5 tests on `SplatConfig` default,
    round-trip, and dirty-flag wiring).
  - **Build a `.sd7` and load in BAR. Confirm:** the painted
    distribution does NOT yet appear in BAR (D6 not done). This is
    expected — the editor preview is decoupled from emission until D6
    wires it. Note the gap in the devlog.
- Final devlog log summarising what shipped + "Sprint 10 = the next
  open Stream-C / Stream-F item (likely C4 metal-spot inspector or
  C5 geo-vent inspector)" handoff note.

Start by running `git status`, then reading the engine fragment shader
at `/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/
GLSL/SMFFragProg.glsl` — the math you'll translate to WGSL is sitting
right there, no need to derive it. Begin with D4 (D5 depends on D4's
upload path).
