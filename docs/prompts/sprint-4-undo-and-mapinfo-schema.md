# Sprint 4 — Undo for non-heightmap state + mapinfo schema model (B5, C1)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 4** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship **B5 + C1** — two foundational data-model items that
unblock the big F8 redesign (B6) and most of Streams C and D.

**Prerequisites:** Sprints 1–3 (A1–A4, B1, B2, B3, B4) should already be
ticked in phase-3-plan.md, with ADR-030 (layout shell), ADR-031 (symmetry
global), and ADR-033 (undo copy-on-first-write) in `docs/DECISIONS.md`.
Verify before starting. If anything earlier is unticked, stop — finish that
first.

This sprint is intentionally a "boring foundation" pair: no visible feature
ships, but B6 / C2–C8 all depend on these two landing cleanly.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (mapinfo three-gate model),
   §2.1 (pitfall list, especially the mapinfo silent-disable entries), §3.2
   (functional reqs).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable rules.
4. `/home/teague/code/BARMapEditor/devlog/README.md`.
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md`.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` — read
   B5 and C1 in full. Also skim B6 (F8 allyteam redesign), C2 (three-file
   emission), and C7 (F9 form editor) so you understand who'll consume what
   you're building.
7. `/home/teague/code/BARMapEditor/docs/research/mapinfo/claude-research-findings.md`
   — the canonical schema source. **Adopt Claude's findings on every
   divergence** (rationale in phase-3-plan.md C1 — Gemini's report has
   fabricated line numbers and an incorrect feature rotation type).
8. `/home/teague/code/BARMapEditor/docs/research/mapinfo/gemini-bar-map-metadata-research-findings.md`
   — skim for the points where it agrees with Claude (those are doubly safe).
9. ADRs 013 (current minimal-emitter notes — C1 supersedes this), 022 (undo
   data model B5 extends), 023 (start positions — B6's data model
   eventually lives here), 033 (undo copy-on-first-write A1 introduced).
10. `crates/barme-core/src/undo.rs` — what B5 extends.
11. `crates/barme-core/src/project.rs` — what C1 adds fields to.
12. `crates/barme-pipeline/src/mapinfo.rs` — current ad-hoc emitter. C1
    DOES NOT rewrite this (that's C2 / Sprint 6); it only adds the data shape.

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new stage-1-undo-non-heightmap
./devlog/log.sh new stage-1-mapinfo-schema
```

Fill each from phase-3-plan.md.

## Step 3 — Scope

In order, one commit per item:

1. **B5 — Undo barriers for non-heightmap state**
   - Unified history entry: `enum HistoryEntry { Heightmap(StampSnapshot),
     Project(ProjectDiff) }` with one stack. Single Ctrl-Z hits whichever
     entry is on top.
   - `ProjectDiff` variants: `PlaceStartPosition`, `MoveStartPosition`,
     `DeleteStartPosition`, `ApplyWizard(WizardSnapshot)`. Apply / revert
     via serde clone + swap.
   - Each variant tracks its own byte cost; cap stays global at the
     existing 100 MB; eviction prefers the largest entry.
   - Existing `barrier()` call sites (procgen, load, new project) clear
     both kinds.
   - Gate undo on `!app.is_dragging_anything` (don't revert a marker
     mid-drag).
   - Wire F8 + wizard call sites to emit `ProjectDiff` entries.
   - No ADR.

2. **C1 — `mapinfo.lua` schema model in barme-core [ADR-028]**
   - New module `crates/barme-core/src/mapinfo_schema.rs` (keep in
     `barme-core`; promote to `crates/barme-mapinfo` later if it exceeds
     ~500 LOC).
   - Data types for every top-level mapinfo table per Claude's research
     digest. The skeleton is in phase-3-plan.md C1 — populate every field
     with the right type from the digest.
   - `MapInfo::bar_default()` constructor returning BAR conventions
     (gravity 130, extractor_radius 80, depend = `["Map Helper v1"]`,
     modtype 3, atmosphere fogStart 0.1 / fogEnd 1.0, splats tex_scales
     `[0.02; 4]` / tex_mults `[1.0; 4]`, etc.).
   - Conversion: `impl From<&Project> for MapInfo`. Reads project state,
     populates the schema. **Don't rewrite the emitter** (C2 / Sprint 6
     does that); just produce the typed struct.
   - `Project` gains `mapinfo_overrides: HashMap<String, toml::Value>`
     (or similar — F9 will populate this; for now an empty default).
   - Writes ADR-028 (supersedes ADR-013's minimal-emitter notes — both
     status-updated in DECISIONS.md).

Then a **3rd rollup commit**: STATUS UPDATEs in SRS / ROADMAP, tick 2 boxes
in phase-3-plan.md, closing devlog log.

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

- B6 (F8 allyteam redesign) — Sprint 5/6. C1 builds the data shape, B6
  consumes it.
- B7, B8 — separate sprints.
- C2 (three-file emission + Lua AST emitter) — Sprint 6. C1 does NOT
  rewrite the emitter; that's a separate item with its own ADR (029).
- C3, C4, C5, C6, C7, C8 — later sprints.
- D / E streams.
- `Project.metal_spots`, `Project.geo_vents`, `Project.features`,
  `Project.ally_groups` — those land in their respective C-stream items
  (C4, C5, C6, B6/C2). Leave them out of C1's scope; only add fields
  C1 actively needs (`mapinfo_overrides`).

## Step 6 — Critical pitfalls (read twice)

From phase-3-plan.md and the research digest:

1. **`teams[]` carries ONLY `startPos`, never `allyTeam`.** Both research
   reports agree. Allyteam membership lives in
   `mapconfig/map_startboxes.lua` (a SEPARATE FILE) and is materialised
   from `Project.ally_groups` at C2's emission time. C1 schema MUST NOT
   model `ally_team` inside `TeamBlock`.

2. **`lighting.sun_dir` is `[f32; 4]` (vec3 + w distance).** Not
   `[f32; 3]`. Easy mistake.

3. **`extractor_radius` BAR convention is 80**, NOT the engine default of
   500. `bar_default()` must use 80.

4. **`fog_start == fog_end` (both 1.0) breaks build ETA.** Default
   atmosphere block must use `fog_start = 0.1`, `fog_end = 1.0`.

5. **`modtype = 3` is the Chobby filter gate.** Without it, the map is
   invisible to BAR's lobby. `bar_default()` must set it.

6. **`depend = ["Map Helper v1"]` is required.** Missing → engine
   fallback rendering ("untextured" symptom). `bar_default()` must include it.

7. **`splat_detail_normal_tex` set without `specular_tex`** silently
   disables. The schema MODELS both; the lint pass (C8) enforces.

8. **Unified undo cap eviction.** Heightmap entries are bytes-heavy
   (MBs); ProjectDiff entries are kilobytes. Sharing a 100 MB global cap
   means a long stroke could evict 20 recent F8 placements. Mitigation:
   eviction prefers the *largest* entry, not strictly the oldest. If
   that feels wrong in practice, escalate (don't silently switch to
   per-channel caps without reopening the decision).

9. **`HistoryEntry::Project(ApplyWizard(...))` snapshots the whole
   project.** Could be large if the project already has many start
   positions / future feature lists. Bound by accounting `mem::size_of_val`
   against the cap.

10. **`serde(default, skip_serializing_if = ...)` on every new field**
    so an upgrade path is open if Recoil adds new fields.

## Step 7 — Exit criteria

- 3 commits on `main`: B5, C1 + rollup.
- 2 devlog folders filled.
- 2 checkboxes ticked in phase-3-plan.md.
- ADR-028 (mapinfo schema) in `docs/DECISIONS.md` — supersedes ADR-013;
  both annotated with STATUS UPDATE.
- SRS § F9 (mapinfo editor) annotated with STATUS UPDATE noting schema
  shipped (form editor still pending in C7).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Place a start position, Ctrl-Z → marker removed.
  - Apply wizard with new size, Ctrl-Z → previous project restored.
  - Long brush stroke followed by 50 start-position placements, Ctrl-Z×51
    returns to pre-stroke state.
  - `MapInfo::bar_default()` produces a struct with `modtype == 3`,
    `extractor_radius == 80`, `depend.contains("Map Helper v1")`,
    `splats.tex_scales == [0.02; 4]`.
  - All 20+ top-level mapinfo fields modelled (cross-check against the
    research digest's schema list).
  - `impl From<&Project> for MapInfo` compiles and round-trips the
    current `Project` shape.
  - Unit tests cover every BAR-default value.
- Final devlog log summarising what shipped + "Sprint 5 = C2 + B6
  (three-file emission convention + F8 allyteam redesign)" handoff note.

Start by running `git status` and reading the files in Step 1. Then begin B5.
