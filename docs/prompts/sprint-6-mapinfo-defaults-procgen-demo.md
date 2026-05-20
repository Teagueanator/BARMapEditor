# Sprint 6 — mapinfo BAR defaults + procgen UX + demo state (C3, B7, B8)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 6** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship **C3 + B7 + B8** — three small finishing-touch items
that bring the editor's "out-of-the-box" experience up to par. None of them
adds new feature surface; they polish what's already there.

**Prerequisites:** Sprints 1–5 (A1–A4, B1–B6, C1, C2) should all be ticked
in phase-3-plan.md, with ADRs 028, 029, 030, 031, 032, 033 in
`docs/DECISIONS.md`. Verify before starting.

C3 depends on the C2 emitter shipping (Sprint 5) — it ONLY adjusts default
values, doesn't rewrite emission. B7 depends on A3/A4 (procgen perf +
syntax check from Sprint 1) and B1 (Inspector). B8 depends on B6 (so demo
state can pre-place ally groups).

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.2 (F1 wizard, F14 procgen),
   §3.3 NFRs. **Read every STATUS UPDATE block dated 2026-05-18 — the
   source-audit one is load-bearing for C3.**
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable rules.
   Pitfalls 11–21 (added 2026-05-18 by the source audit) are direct C3
   inputs: case-sensitive sun direction keys, deprecated `skyDir`, all-zero
   metalmap, default value corrections, etc.
4. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`**
   — the ground-truth mapinfo schema reconciled against the actual
   `RecoilEngine` and `Beyond-All-Reason` source. §1 is the per-field
   default table you'll use. §12 enumerates the new pitfalls. **This
   supersedes the older Claude/Gemini digests where they conflict.**
5. `/home/teague/code/BARMapEditor/devlog/README.md`.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md`.
7. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` —
   read C3, B7, B8 in full.
8. `/home/teague/code/BARMapEditor/docs/research/mapinfo/claude-research-findings.md`
   — sections on BAR-default values for lighting (sunDir), atmosphere
   (minWind/maxWind/fog), water (when to populate), splats (texScales /
   texMults), terrainTypes. **Treat as background; defer to FINDINGS.md
   above where the two disagree** (the audit caught several wrong defaults
   in this digest — `sunDir.w = 1.0` not `1e9`, `water.baseColor = (0,0,0)`
   not `(0.4,0.7,0.8)`, `water.minColor` similarly, etc.).
9. `/home/teague/code/BARMapEditor/docs/research/ui/claude-research-findings.md`
   — sections on procgen redesign (preview thumbnail, preset-first order)
   and demo state (non-empty initial terrain + framing hint).
10. `/home/teague/code/BARMapEditor/docs/research/ui/Gemini UX Redesign for BAR Map Editor.md`
    — Gemini's "Shippable Demo State" framing.
11. ADRs 020 (procgen — B7 redesigns the UI surface), 024 (wizard — B8
    extends), 028 (schema — C3 populates defaults), 029 (emission — C3
    output flows through this).
12. **Direct source references** (clones under `/home/teague/code/`):
    - `RecoilEngine/rts/Map/MapInfo.cpp:127-172` — atmosphere defaults.
    - `RecoilEngine/rts/Map/MapInfo.cpp:201-225` — lighting defaults.
    - `RecoilEngine/rts/Map/MapInfo.cpp:228-336` — water defaults.
    - `Beyond-All-Reason/luarules/gadgets/unit_sunfacing.lua:43` —
      the lowercase `sundir` read.
13. `crates/barme-core/src/mapinfo_schema.rs` (from C1) — where C3 writes
    defaults.
14. `crates/barme-core/src/procgen.rs` — what B7's preview helper extends.
15. `crates/barme-app/src/main.rs` — where the wizard's `apply_wizard`
    method lives; B8 extends it.

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new stage-1-mapinfo-defaults
./devlog/log.sh new stage-1-ux-procgen-redesign
./devlog/log.sh new stage-1-ux-demo-state
```

Fill each from phase-3-plan.md.

## Step 3 — Scope

In order, one commit per item:

1. **C3 — Full mapinfo block expansion (lighting / atmosphere / resources / water / splats / terrainTypes)**

   **READ FIRST** —
   `docs/research/source-audit-2026-05-18/FINDINGS.md` §1 has the
   ground-truth defaults from `RecoilEngine/rts/Map/MapInfo.cpp` at HEAD.
   Use that table; the older Claude / Gemini digests have several stale
   defaults the audit caught.

   - Populate `MapInfo::bar_default()` with the full BAR-default value set
     from the source-audit findings:
     - **`lighting` block** — `groundAmbientColor`, `groundDiffuseColor`,
       `groundSpecularColor`, `groundShadowDensity`, `unitAmbientColor`,
       `unitDiffuseColor`, `unitSpecularColor`, `unitShadowDensity`,
       `specularExponent` per digest defaults.
     - **`lighting.sunDir`** is `[f32; 4]` defaulting to
       `[0.5, 0.7, 0.5, 1.0]` (or any normalized direction with **w =
       1.0**, NOT `1e9`). Engine code: `MapInfo.cpp:207` =
       `float4(0.0f, 1.0f, 2.0f, 1.0f)`.
     - **EMIT BOTH `sunDir` AND `sundir`** with the same value (see
       PITFALL #11; engine reads camelCase, BAR's `unit_sunfacing.lua`
       reads lowercase). The two are distinct Lua keys.
     - `atmosphere.minWind = 5.0`, `maxWind = 25.0`, `fogStart = 0.1`,
       `fogEnd = 1.0` (**not both 1.0** — breaks build ETA), plus
       `fogColor = (0.7, 0.7, 0.8)`, `sunColor = (1, 1, 1)`,
       `skyColor = (0.1, 0.15, 0.7)`, `cloudColor = (1, 1, 1)`,
       `cloudDensity = 0.5`, `fluidDensity = 0.3`.
     - **`atmosphere.skyAxisAngle = [0.0, 0.0, 1.0, 0.0]`** —
       float4, xyz axis + radians angle. **DO NOT EMIT `skyDir`** — the
       engine logs `L_DEPRECATED` and prefers `skyAxisAngle` (PITFALL #12).
     - `splats.tex_scales = [0.02; 4]`, `tex_mults = [1.0; 4]`.
     - `terrain_types = vec![…]` — BAR ships 4 default types; populate.
     - `resources.*` left as `None` here (Stream D's D6 wires them when
       splat painting lands).
     - `water` populated as `None` by default; constructor takes a
       `tidal_strength > 0 || min_height < 0` flag to opt in. **If
       populated, use the audited defaults from FINDINGS §1.5** —
       `baseColor = (0,0,0)`, `minColor = (0,0,0)`, `surfaceColor =
       (0.75, 0.8, 0.85)`, `planeColor = (0.0, 0.4, 0.0)`, `surfaceAlpha
       = 0.55`, `numTiles = 4` (when no custom normal). The older
       research's `baseColor = (0.4, 0.7, 0.8)` is **wrong**.
     - **`map.voidAlphaMin = 0.9`** — new field flagged by audit; add to
       the `map` block.
     - **`modtype`** must serialize to integer 3 (`Modtype::Map`).
   - **REGRESSION TEST INVERSION:** the existing C3 emitter test
     reportedly asserts that lowercase `sundir` does NOT leak into the
     rendered output. That test is **incorrect** per the audit; flip it.
     The new contract: the emitter renders BOTH `sundir` and `sunDir`
     keys in `lighting`, both with the same value. Add a test:
     ```rust
     #[test]
     fn lighting_emits_both_sundir_cases() {
         let rendered = render_mapinfo(&MapInfo::bar_default());
         assert!(rendered.contains("sundir = "));
         assert!(rendered.contains("sunDir = "));
     }
     ```
   - **Source the `gui` block from `MapInfo.cpp:119-124` only** — only
     `autoShowMetal` is read. Drop `gui.minimapRotation` if it appears
     in the schema (PITFALL #19).
   - Unit tests asserting each field's BAR-default value. The tests are
     the deliverable — if a future refactor drifts a default, the test
     catches it.
   - No ADR.

2. **B7 — Procgen UX redesign (preview + preset-first order)**
   - Reorder the Procgen Inspector section (visible when
     `Tool::Procgen` active, per B1):
     1. Preset dropdown (Flat / Parabolic bowl / Cone peak / Diagonal
        ramp / Sine ripples).
     2. Custom expression in a `CollapsingHeader` (`"Custom expression"`,
        collapsed by default).
     3. Domain radio ([0,1] / [-1,1]).
     4. 256×256 preview thumbnail (`egui::TextureHandle` + `ui.image`).
     5. Commit button (`"Apply to heightmap"`) — disabled when parse fails
        (A4 wired this).
   - Preview rendering: `procgen::generate_thumbnail(expr, domain, 256)`
     helper that's just `generate` with a small fixed size. Map to
     greyscale `ColorImage`. Re-upload to a persistent `TextureHandle`
     (use `handle.set(image, TextureOptions::default())` — never recreate
     the handle).
   - 50 ms debounce on parse + thumbnail update: track `last_changed_at:
     Instant` on `App`; only re-eval when `now - last_changed_at >= 50ms`.
     Use `ctx.request_repaint_after(...)` to wake on debounce.
   - No ADR.

3. **B8 — Pre-populated demo state on wizard close**
   - Modify the wizard's "Create" handler (`App::apply_wizard`) so post-
     Create state is:
     - Heightmap from the chosen biome preset (existing behaviour).
     - Symmetry set to Horizontal by default.
     - 2 default start positions placed in `ally_groups[0]` (B6's data
       model):
       - `(map_center_x, map_extents_z * 0.15)` — north strip.
       - `(map_center_x, map_extents_z * 0.85)` — south strip.
     - Camera framed at 35° pitch, ~1.6× diagonal distance from map
       centre.
   - Open a non-modal "Next steps" `egui::Window` overlay after wizard
     close. Three bullets:
     1. "Brush terrain → press B, then click-drag in the viewport"
     2. "Move spawns → press S, then drag the markers"
     3. "Try a math preset → press G, choose Parabolic bowl, click Apply"
   - Dismiss with X button. Dismissal persists per-project (write
     `next_steps_dismissed: true` to the `.barmeproj` file — not the
     per-user config, since reopening another fresh project should re-show
     the hint).
   - No ADR.

Then a **4th rollup commit**: STATUS UPDATEs in SRS / ROADMAP, tick 3
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

- C4 / C5 / C6 / C7 / C8 — later sprints. C3 populates schema defaults
  only; it does NOT populate `Project.metal_spots`, `geo_vents`,
  `features`, or run a linter.
- D-stream / E-stream.
- F12 launch button — wizard's "Launch demo map" is a future ask.
- Preset library expansion (Crater, Mesa, Simplex) — Stage 2.

## Step 6 — Critical pitfalls (read twice)

1. **`atmosphere.fogStart == fogEnd` (both 1.0)** breaks the build-ETA
   grid renderer. Test asserts `fogStart != fogEnd`.

2. **`lighting.sunDir` is `[f32; 4]`**, camelCase key (engine reads
   `sunDir`), w defaults to **`1.0` not `1e9`**. AND **EMIT
   `lighting.sundir` (lowercase) AS A SECOND KEY** — BAR's
   `unit_sunfacing.lua` (`Beyond-All-Reason/luarules/gadgets/
   unit_sunfacing.lua:43`) reads lowercase. Engine + gadget read
   different Lua keys; both must be present. Lua tables are
   case-sensitive — `sunDir` and `sundir` are distinct entries.

3. **`atmosphere.skyDir` is DEPRECATED.** Engine logs
   `L_DEPRECATED` at `MapInfo.cpp:144-146` if you set it. Use
   `atmosphere.skyAxisAngle = {axisX, axisY, axisZ, angleRadians}`
   instead.

4. **Thumbnail handle reuse**: re-uploading a fresh `ColorImage` is fine,
   but creating a new `TextureHandle` per parse leaks GPU memory. Use
   `handle.set(...)` on a stored `Option<TextureHandle>`.

5. **Domain change re-evaluates**: the thumbnail must redraw on domain
   toggle, not just expression change. Hash `(expr, domain)` together
   for the debounce key.

6. **Demo-state start positions in terrain valleys**, not peaks. Some
   biomes peak at the centre (Parabolic dome). Heuristic: place markers
   where normalised heightmap height ∈ `[0.2, 0.6]`. If both default
   coords fall outside that range, fall back to the map quarter-points.

7. **B8 dismiss flag**: per-project (in `.barmeproj`), NOT per-user (in
   config TOML). Per-user would suppress the hint forever after first
   dismiss; that's wrong for an editor where users open many fresh
   projects.

8. **`terrain_types`** is an indexed table 0..3 with BAR conventions. Per
   digest: index 0 = Default, hardness 1.0, all moveSpeeds 1.0. Don't
   reinvent — copy the digest's defaults.

9. **B7 preview greyscale mapping**: heightmap u16 → greyscale u8 is
   `(value * 255 / 65535) as u8`. Watch for integer overflow if reusing
   `procgen::generate` directly — its output is f32 normalised, easier to
   map.

10. **Water defaults drifted in older research** — re-read
    `RecoilEngine/rts/Map/MapInfo.cpp:228-336` for the ground truth.
    Notable: `baseColor` / `minColor` default `(0,0,0)` not the
    (0.4,0.7,0.8) / (0.1,0.2,0.3) shown in earlier research.
    `numTiles` defaults to 4, not 1. `planeColor` has a default
    `(0, 0.4, 0)` — not "unset".

11. **`gui.minimapRotation` is unused** — `ReadGui` only consumes
    `autoShowMetal`. Don't emit `minimapRotation`.

12. **`map.voidAlphaMin = 0.9`** is a missing field from older research;
    add it to the `map` block default set.

## Step 7 — Exit criteria

- 4 commits on `main`: C3, B7, B8 + rollup.
- 3 devlog folders filled.
- 3 checkboxes ticked in phase-3-plan.md.
- No new ADRs.
- SRS / ROADMAP STATUS UPDATEs (mapinfo defaults shipped, procgen UX
  redesigned, demo state shipped).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - `MapInfo::bar_default()` returns a struct with `gravity == 130.0`,
    `extractor_radius == 80.0`, `modtype == 3`, `atmosphere.fog_start ==
    0.1`, `atmosphere.fog_end == 1.0`, `atmosphere.sky_axis_angle ==
    [0.0, 0.0, 1.0, 0.0]` (NOT `sky_dir`), `splats.tex_scales ==
    [0.02; 4]`, `terrain_types.len() == 4`, `lighting.sun_dir[3] ==
    1.0` (the w component, NOT 1e9), `map.void_alpha_min == 0.9`.
  - Rendered `mapinfo.lua` contains BOTH `sundir = ` and `sunDir = `
    keys under `lighting`. Test asserts substring presence of both.
  - Rendered output does NOT contain `skyDir =` or `minimapRotation =`.
  - Build an "empty" project → loaded `.sd7` in BAR shows no untextured /
    broken-fog / crashed-gadget regressions.
  - Procgen tool: switching presets updates the thumbnail in <100 ms.
  - Typing `x*z` shows live thumbnail update; typing `x*2x` keeps prior
    thumbnail and shows red ✗.
  - Wizard → Create → terrain visible, 2 markers visible (north + south
    strips), camera framed at pitch 35°, Next-steps Window visible.
  - Dismiss Next-steps; reopen project; window stays dismissed.
  - Open a fresh project; window reappears.
- Final devlog log summarising what shipped + "Sprint 7 = D1 (texture
  pack decision)" handoff note. Mention that Sprint 7 is independent of
  Sprints 5/6 — could have run in parallel.

Start by running `git status`, then reading the files in Step 1. Begin with
C3 (smallest, no dependency on the others).
