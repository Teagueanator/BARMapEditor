# Session prompts

One file per session — each prompt is self-contained and copy-pasteable
into a fresh Claude / Claude Code session. The new session reads its own
context (no reliance on chat history or memory carry-over beyond the
auto-memory under `~/.claude/projects/-home-teague-code-BARMapEditor/`).

Sequence mirrors the sprint table in
`devlog/stage-1-mvp/phase-3-plan.md` § "Order of attack." Each prompt
declares which item IDs (A1, B1, C1, ...) it ships; the plan file is
the canonical source for *what* each item entails.

| Sprint | Prompt | Items | Theme |
|---|---|---|---|
| 1 | [sprint-1-bug-triage.md](sprint-1-bug-triage.md) | A1, A2, A3, A4 | Stream A complete — kill the known bugs |
| 2 | [sprint-2-layout-shell.md](sprint-2-layout-shell.md) | B1 | Five-zone layout + tool-mode left strip (ADR-030) |
| 3 | [sprint-3-canvas-and-symmetry.md](sprint-3-canvas-and-symmetry.md) | B2, B3, B4 | Symmetry global + canvas affordances + top-bar build |
| 4 | [sprint-4-undo-and-mapinfo-schema.md](sprint-4-undo-and-mapinfo-schema.md) | B5, C1 | Undo for non-heightmap state + mapinfo schema model (ADR-028) |
| 5 | [sprint-5-mapinfo-emission-and-f8.md](sprint-5-mapinfo-emission-and-f8.md) | C2, B6 | Three-file Lua AST emission + F8 allyteam tree (ADR-029, ADR-032) |
| 6 | [sprint-6-mapinfo-defaults-procgen-demo.md](sprint-6-mapinfo-defaults-procgen-demo.md) | C3, B7, B8 | BAR-default mapinfo + procgen UX + demo state on wizard close |
| 7 | [sprint-7-texture-pack-decision.md](sprint-7-texture-pack-decision.md) | D1 | Starter texture pack decision + fetch script (ADR-025, ADR-027). **Independent of 5–6 — can run in parallel.** |
| 8 | [sprint-8-dnts-bake-and-splat-module.md](sprint-8-dnts-bake-and-splat-module.md) | D2, D3 | DNTS bake pipeline (Y-flip, Compressonator) + splat module (ADR-026) |
| 9 | [sprint-9-splat-shader-and-ui.md](sprint-9-splat-shader-and-ui.md) | D4, D5 | Splat fragment shader (ADR-036 with source-audit corrections) + splat tool UI |
| 10 | [sprint-10-mapinfo-audit-fix.md](sprint-10-mapinfo-audit-fix.md) | Audit fix | Mapinfo emitter corrections per PITFALL §11/12/18/19/20 (sundir+sunDir dual emit, skyAxisAngle, drop minimapRotation, fix sunDir.w, add voidAlphaMin). |
| 11 | [sprint-11-metal-and-geo.md](sprint-11-metal-and-geo.md) | C4, C5 | F5 metal-spot tool + F6 geo-vent tool + black metalmap PNG |
| 12 | [sprint-12-features-and-splat-emission.md](sprint-12-features-and-splat-emission.md) | C6, D6 | F7 stock-feature placement + splat .sd7 wiring (DNTS DDS bake + splat distribution PNG + mapinfo.resources subtable) |
| 13 | [sprint-13-renderer-depth-rework.md](sprint-13-renderer-depth-rework.md) | (renderer foundation) | **Foundation of the renderer-parity arc.** Offscreen RT + depth attachment + GPU markers pipeline. ADR-037. |
| 14 | [sprint-14-water-and-lava.md](sprint-14-water-and-lava.md) | C9 | Water + Lava (MVP) — `Tool::Water`, presets, flat alpha-blended water plane at Y=0. ADR-042. (Polish in Sprint 26.) |
| 15 | [sprint-15-layered-painter-data-model.md](sprint-15-layered-painter-data-model.md) | D8 | Layered painter, Part 1 — data model + CPU bake. ADR-038. |
| 16 | [sprint-16-layered-painter-paint-and-composite.md](sprint-16-layered-painter-paint-and-composite.md) | D9 | Layered painter, Part 2 — 2D viewport + GPU composite + tiled-COW masks + mask brushes. ADR-039 + ADR-040. |
| 17 | [sprint-17-layered-painter-ui-and-bake.md](sprint-17-layered-painter-ui-and-bake.md) | D10 | Layered painter, Part 3 — Layers panel + DNTS hybrid emission + retire `Tool::SplatPaint` + `inspector_splat`. ADR-041. |
| 18 | [sprint-18-minimap-and-form-editor.md](sprint-18-minimap-and-form-editor.md) | D7, C7 | F10 minimap auto-generation + F9 schema-driven mapinfo form editor. **Sprint 17 followups (16-SMU OOM, orphan-texture GC, legacy `SplatConfig`) deferred to Sprint 23.** |
| 19 | [sprint-19-tooltip-and-help-text-pass.md](sprint-19-tooltip-and-help-text-pass.md) | U1 | **UI polish #1 of 3** — adds 80+ missing tooltips + `help_text` catalogue + validation-chip click affordance + status-strip "0 issues" wiring + persistent Help (?) icon + false-promise tooltip cleanup + brush-ring colour fix. |
| 20 | [sprint-20-async-build-and-log.md](sprint-20-async-build-and-log.md) | U3 + F11 polish | **UI polish #2 of 3** — async `build_and_install` on worker thread + in-app build log panel + progress overlay with cancel + recent projects list + save-before-build guard. |
| 21 | [sprint-21-lint-pass.md](sprint-21-lint-pass.md) | C8 | Lint My Map pass — 30+ rules covering PITFALLS §1–28; populates Sprint 19's lint panel stub; build-button gates on errors only. **Stage 1 internal-work complete except F12.** |
| 22 | [sprint-22-onboarding-and-command-palette.md](sprint-22-onboarding-and-command-palette.md) | U2 | **UI polish #3 of 3** — help center + 28+ articles + 7-step guided tour + per-tool intro overlays + Ctrl-K command palette + "What's this?" hover-popover mode. |
| 23 | [sprint-23-painter-cleanup-and-oom.md](sprint-23-painter-cleanup-and-oom.md) | T1 | Sprint-17 cleanup — 16-SMU `Tool::PaintLayer` OOM root-cause + orphan imported-texture GC + runtime retirement of legacy `SplatConfig`. |
| 24 | [sprint-24-multithreading-procgen-dnts.md](sprint-24-multithreading-procgen-dnts.md) | T2 | Multithreading — rayon procgen apply (~3.5× speedup at 16-SMU) + parallel DNTS bake (per-slot par_iter). |
| 25 | [sprint-25-terrain-shader-parity.md](sprint-25-terrain-shader-parity.md) | R1 | **Renderer-parity 1/8** — port `SMFFragProg.glsl` to WGSL. DNTS composite, TBN, base normal R+A, specular exponent. Subsumes Sprint 9 / D4 placeholder. ADR-038 (new). |
| 26 | [sprint-26-water-polish.md](sprint-26-water-polish.md) | R3 | **Renderer-parity 2/8** — water polish: planar reflection, refraction, fresnel, foam, caustics, perlin waves, lava emission glow. Supersedes Sprint 14's MVP. ADR-039 (new). |
| 27 | [sprint-27-inspector-consistency-refactor.md](sprint-27-inspector-consistency-refactor.md) | U5 | Inspector consistency refactor — lift `brush_card` widget, sticky symmetry+mapsize chip, standardised delete buttons, section-pattern enforcement across all 9 tools, deduplicate height_scale. |
| 28 | [sprint-28-atmosphere-and-fog.md](sprint-28-atmosphere-and-fog.md) | R2 | **Renderer-parity 3/8** — exponential height fog, sun colour ramp, sky-color background, skybox cubemap, wind state for water + grass. ADR-040 (new). |
| 29 | [sprint-29-feature-asset-decoding.md](sprint-29-feature-asset-decoding.md) | R5 | **Renderer-parity 4/8** — feature decals (Phase A; mandatory) and/or S3O parsing (Phase B; stretch). Trees, rocks, wreckage become visually correct. |
| 30 | [sprint-30-directional-shadows.md](sprint-30-directional-shadows.md) | R4 | **Renderer-parity 5/8** — single-cascade shadow map from sun direction. Terrain + features cast and receive. PCF for soft edges. ADR-041 (new). |
| 31 | [sprint-31-toasts-and-confirmation-modals.md](sprint-31-toasts-and-confirmation-modals.md) | U4 | Toast queue (Info / Warn / Error) + confirmation modal primitive. Migrates ~12 `last_error` sites; wires confirms on delete-ally-group / delete-layer / new-project-with-unsaved. |
| 32 | [sprint-32-launch-in-bar-and-autosave.md](sprint-32-launch-in-bar-and-autosave.md) | T5 + F12 | F12 Launch in BAR (`Command::spawn` with `--map`) + autosave 60s (NFR-Crash-safety) + recovery prompt + Settings UI. **Stage 1 + F12 complete; external Beherith review next.** |
| 33 | [sprint-33-nfr-ci-gates.md](sprint-33-nfr-ci-gates.md) | T6 | Beta-prep CI — MSRV matrix, criterion benches in CI, `.sd7` end-to-end determinism test, Windows + macOS build, Linux AppImage, headless wgpu CI. |
| 34 | [sprint-34-grass-rendering.md](sprint-34-grass-rendering.md) | R6 | **Renderer-parity 6/8** — grass blades as instanced quads, wind sway, density-from-terraintype, camera-distance LOD. ADR-043 (new). |
| 35 | [sprint-35-emission-skybox-parallax.md](sprint-35-emission-skybox-parallax.md) | R7 | **Renderer-parity 7/8** — `lightEmissionTex`, `skyReflectModTex`, `parallaxHeightTex` (verify first), `grassBladeTex`. ADR-044 (new). |
| 36 | [sprint-36-parity-validation.md](sprint-36-parity-validation.md) | R8 | **Renderer-parity 8/8** — automated ΔE harness; 3-map × 3-angle validation suite; SRS §2.1 #11 closeout. ADR-045 (new). |
| 37 | [sprint-37-brushes-and-symmetry-line.md](sprint-37-brushes-and-symmetry-line.md) | F2/F3 closeout | Flatten / erode / ramp brushes + arbitrary-axis symmetry line picker. First Stage-2 sprint. Closes SRS F2 + F3. |
| 38 | [sprint-38-mapfeatures-autogen-and-extra-lints.md](sprint-38-mapfeatures-autogen-and-extra-lints.md) | L2 + autogen | Catalog auto-generation from upstream `mapfeatures` repo + PITFALLS §22-28 lint rule coverage. |
| 39 | [sprint-39-user-asset-library.md](sprint-39-user-asset-library.md) | F23 | User-asset library — heightmap stamps + feature prefabs + DNTS material packs, stored in XDG data dir, indexed by tags. |
| 40 | [sprint-40-sd7-import.md](sprint-40-sd7-import.md) | F13 | `.sd7` decompile / import — reverse the build pipeline; reconstruct an editable Project from any BAR map. |
| 41 | [sprint-41-procgen-v2-fbm-and-river-carve.md](sprint-41-procgen-v2-fbm-and-river-carve.md) | F14 v2 | FBM (Fractional Brownian Motion) noise primitive + river-carve interactive line brush. Closes SRS F14. |
| 42 | [sprint-42-typemap-editor.md](sprint-42-typemap-editor.md) | F15 | Type-map editor `Tool::TypeMap` + per-terraintype gameplay params editing in F9 form. Wires Sprint 34 grass density to real type_map. |
| 43 | [sprint-43-skybox-library.md](sprint-43-skybox-library.md) | F16 | Skybox picker gallery + atmospheric preset library (BrightDay / Sunset / Overcast / Night / etc.). |
| 44 | [sprint-44-pathability-overlay.md](sprint-44-pathability-overlay.md) | F17 | Pathability overlay — colour-codes per-pixel by which locomotor classes can traverse. Closes Stage-2 F-list. |
| 45 | [sprint-45-theme-toggle-and-status-bar.md](sprint-45-theme-toggle-and-status-bar.md) | F21 + F22 | Light/dark theme toggle + live CPU% / RAM status chips. After this, Stage 2 F-list is **effectively complete**. |

## Stream cheat sheet (Sprints 18–37)

**UI polish trio** (19 / 20 / 22): tooltip pass → async build + log → onboarding +
tour + command palette. Each builds on the prior — discoverable widgets first, then
async feedback for long operations, then guided onboarding that reuses the catalogue.

**Renderer-parity arc** (Sprints 25, 26, 28, 29, 30, 34, 35, 36 — 8 sprints). The
user reversed SRS §2.1 #11 on 2026-05-18: the editor must reproduce BAR's render.
Sprint 13 shipped the offscreen+depth foundation; Sprint 25 starts the
shader port. Sprint 36 closes the arc with the ΔE validation suite. Detailed
roadmap: [`docs/research/renderer-bar-parity/ROADMAP.md`](../research/renderer-bar-parity/ROADMAP.md).

**Tech debt sprints** (23, 24, 27, 31, 33): painter cleanup → multithreading →
inspector refactor → toasts/confirms → NFR/CI gates. Each closes one or more
NFR commitments from the SRS.

**MVP completion** (32): F12 Launch in BAR + autosave finally close the
F-list. After this, Stage 1 internal work is complete pending Beherith review.

**Stage 2 starts** (37): the F-brushes (flatten/erode/ramp) + arbitrary-axis
symmetry line picker close the SRS-promised F2/F3 deliverables that didn't
ship in the original sprints.

## Vulkan / cross-platform compatibility

The renderer-parity arc (25–36) ports BAR's GLSL into **WGSL** — we never
write Vulkan SPIR-V by hand. wgpu compiles WGSL to the native backend per
OS:

- Linux / Windows → Vulkan SPIR-V (or D3D12 DXIL on Windows / GL fallback).
- macOS → Metal MSL.
- Future web build → WebGPU / WebGL2.

Every renderer prompt (25, 26, 28, 30, 34, 35) includes a "platform-
portability checklist" — WGSL only, no backend-specific extensions, test
on Vulkan + GL fallback. Sprint 33 lands CI for all three desktop
platforms.

## How to use a prompt

1. Open the relevant `sprint-N-*.md` file.
2. Select-all, copy.
3. Paste as the first user message of a fresh Claude / Claude Code session.
4. The session will read context from its Step 1 list, then execute.

## When to add a new prompt

Each compact boundary that opens a new session needs a new prompt unless
the prior sprint's last devlog log already includes a "next session"
section detailed enough to act as one. When sprints get smaller (one
item per session), it's often cheaper to script the prompt
template-wise — see any existing file for the structure.
