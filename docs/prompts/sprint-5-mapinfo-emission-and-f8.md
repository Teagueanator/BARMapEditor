# Sprint 5 — Three-file mapinfo emission + F8 allyteam redesign (C2, B6)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 5** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship **C2 + B6** — the Lua AST emitter + three-file emission
convention (mapinfo.lua + map_startboxes.lua + map_metal_layout.lua +
featureplacer/features.lua), and the F8 allyteam tree UI on top. Together
they're the marquee fix that finally encodes 8v8 / 3-way FFA / 4-way FFA
correctly in the `.sd7`.

**Prerequisites:** Sprints 1–4 (A1–A4, B1–B5, C1) should all be ticked in
phase-3-plan.md, with ADRs 028, 030, 031, 033 in `docs/DECISIONS.md`.
Verify before starting. C2 depends on C1's `MapInfo` struct existing in
`crates/barme-core/src/mapinfo_schema.rs`. B6 depends on B5's
`ProjectDiff` undo channel.

This is a heavy session — bundled because C2 and B6 are tightly coupled
(B6's `Project.ally_groups` data shape MUST match what C2's emitter
flattens into `teams[]` + `map_startboxes.lua` polygons). Splitting them
risks landing one half with the wrong contract.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (mapinfo three-gate model),
   §2.1 (pitfall list), §3.2 (F8 status update).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable rules.
4. `/home/teague/code/BARMapEditor/devlog/README.md`.
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md`.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` — read
   **C2 and B6 in full**. Also re-read C1 (Sprint 4) so you know what
   schema you're emitting, and skim C4 / C5 / C6 so you understand who'll
   consume the sidecar emitters you set up.
7. `/home/teague/code/BARMapEditor/docs/research/mapinfo/claude-research-findings.md`
   — sections on `teams[]` flat pool, allyTeams encoding via
   `map_startboxes.lua`, feature rotation as string-quoted Spring
   heading, three-file convention. **Adopt Claude on every divergence**
   (Gemini's report has fabricated line numbers and an incorrect feature
   rotation type).
8. `/home/teague/code/BARMapEditor/docs/research/ui/claude-research-findings.md`
   — section on the AllyTeam → Position tree, drag-paint, hover-pulse
   feedback, configuration presets (1v1, 8v8, FFA).
9. `/home/teague/code/BARMapEditor/docs/research/ui/Gemini UX Redesign for BAR Map Editor.md`
   — skim for derived-position visual treatment (greyed in tree).
10. ADRs 013 (string-concat emitter — being replaced), 019 (symmetry —
    still in play for derived positions), 022/033 (undo), 023 (current
    flat `start_positions` — being refactored), 028 (schema model — C1's
    deliverable), 029 (THIS sprint — three-file convention), 032 (THIS
    sprint — allyteam redesign).
11. `crates/barme-pipeline/src/mapinfo.rs` — current ad-hoc emitter. C2
    rewrites this.
12. `crates/barme-core/src/mapinfo_schema.rs` — C1's output. C2 consumes it.
13. `crates/barme-core/src/project.rs` and `crates/barme-core/src/start_pos.rs`
    — B6 refactors both.
14. `crates/barme-pipeline/src/package.rs` — the `.sd7` staging tree. C2's
    three new files go through here.

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new stage-1-mapinfo-emit-refactor
./devlog/log.sh new stage-1-ux-f8-allyteam
```

Fill each from phase-3-plan.md.

## Step 3 — Scope

In order, one commit per item:

1. **C2 — Three-file emission convention + Lua AST emitter [ADR-029]**
   - Replace the string-concat emitter at `crates/barme-pipeline/src/mapinfo.rs`
     with a Lua AST emitter:

     ```rust
     pub enum LuaKey { Str(String), Int(i64) }
     pub enum LuaValue {
         Nil, Bool(bool), Int(i64), Float(f64), Str(String),
         Table(Vec<(LuaKey, LuaValue)>),
     }
     pub fn serialize(v: &LuaValue) -> String;
     pub fn render_mapinfo(info: &MapInfo) -> String;
     ```

     2-space indent, trailing commas, keys sorted in a canonical order so
     diffs are friendly (NFR-Audit). Integer-keyed tables use `[N] = …`
     form; array-style only where ordering doesn't matter (feature lists).
     Escape `\` and `"`; emit `\n` for newlines in `description`.

   - Three NEW sidecar files at archive root (in addition to `mapinfo.lua`):
     - `mapconfig/map_metal_layout.lua` — `return { spots = {…}, geos = {…} }`.
       Empty placeholder this sprint (`spots = {}`, `geos = {}`); C4/C5
       populate them.
     - `mapconfig/map_startboxes.lua` — `return { startboxes = { [0] = …, [1] = … } }`.
       Populated from `Project.ally_groups[*].box_polygon` per B6. Empty
       placeholder if `ally_groups.len() <= 1`.
     - `mapconfig/featureplacer/features.lua` — `return { … }`. Empty
       placeholder this sprint; C6 populates.

   - Split `crates/barme-pipeline/src/mapinfo.rs` into 4 sibling modules:
     `mapinfo.rs`, `metal_layout.rs`, `startboxes.rs`, `featureplacer.rs`.
     (Promote to a `crates/barme-mapinfo` crate if the combined LOC
     exceeds ~700; judgement call.)

   - Wire all 4 files into `barme-pipeline::package`'s staging tree
     before 7z.

   - Writes ADR-029. Supersedes ADR-013's string-concat approach (status
     update ADR-013).

2. **B6 — F8 allyteam tree + drag-paint + presets [ADR-032]**
   - Refactor `Project`:

     ```rust
     pub struct StartPosition { pub x_elmo: i32, pub z_elmo: i32 }
     pub struct AllyGroup {
         pub id: u8,
         pub name: String,
         pub color: Color32,
         pub start_positions: Vec<StartPosition>,
         pub box_polygon: Option<Vec<(f32, f32)>>,  // 0..1, → map_startboxes.lua
     }
     pub struct Project {
         pub ally_groups: Vec<AllyGroup>,
         // legacy `start_positions` field removed
     }
     ```

   - Backwards-compat: a custom `serde` deserializer (or
     `#[serde(deserialize_with = …)]`) reads pre-Phase-3 `.barmeproj`
     files that still have flat `start_positions: Vec<StartPosition>` and
     materializes them into `ally_groups[0]` with default colour /
     name `"AllyGroup 0"`. Write a fixture test for this migration.

   - `start_pos::assign_team_ids` adapts: per-ally-group parity instead of
     global. Preserve the BAR-side even/odd contract for backwards-compat
     tests; new tests cover the multi-allygroup case.

   - C2 emission consumes `ally_groups`: flattens into `teams[]` by
     walking ally groups in id order and concatenating their positions.
     `box_polygon` (where set) emits into `map_startboxes.lua`.

   - **Inspector tree** (StartPositions tool active — Tool::StartPositions
     was already added in B1):
     - Top: configuration preset dropdown (`Custom`, `1v1`, `8v8`,
       `3-way FFA`, `4-way FFA`). Selecting a preset replaces
       `ally_groups` with a default layout.
     - Tree: `CollapsingHeader` per AllyGroup with colour swatch
       (`color_edit_button_srgba` — persistent `Id` per group to survive
       tool switches) + name TextEdit + position count + delete. Child
       StartPosition rows show index + coords + multi-select checkbox
       + delete.
     - "Add AllyGroup" button at the bottom.

   - **Canvas interaction**:
     - LMB-click empty → place a position in the currently-active ally
       group.
     - LMB-drag empty → distribute N evenly-spaced positions along the
       drag vector. N defaults to 8 (lives in a Inspector `DragValue`).
     - LMB-drag marker → move it.
     - RMB-click marker → delete.

   - **Symmetry interaction**: positions placed in active ally group
     mirror into derived positions. Mirror placement strategy: derived
     positions go into THE SAME ally group (so a Quad symmetry on group 0
     produces 4 positions in group 0, not 4 separate groups). Derived
     positions render greyed in the tree with `(mirror of Pos N)` label;
     they're recomputed on every frame from the source (not stored).
     Editing the source moves the derived; editing a derived position
     shows a tooltip "Edit the source in this group."

   - **Hover↔pulse**: hover Inspector row → marker pulses (2 Hz, 1 s,
     thick ring). Hover marker → Inspector scrolls to the row.

   - **Marker labels**: position index + faction-swatch dot (from
     `AllyGroup.color`). Default palette: Armada blue, Cortex red, Legion
     green, Raptors yellow, plus 4 fallback colours.

   - **Cross-tool visibility**: markers ghosted (50% alpha, no hover
     response) when StartPositions is not the active tool — uses B1's
     pattern.

   - All F8 edits emit `ProjectDiff` undo entries (B5's machinery).
     Verify with `Ctrl-Z` on a placed position.

   - Writes ADR-032. Supersedes ADR-023's flat-vec approach (status
     update ADR-023).

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

- B7 / B8, C3 / C4 / C5 / C6 / C7 / C8 — later sprints.
- D-stream / E-stream.
- Populating the metal/geo sidecar bodies (C4 / C5) — emit empty placeholders this sprint.
- Populating featureplacer/features.lua body (C6) — empty placeholder.
- Per-position colour override — defer; AllyGroup.color is enough.
- Custom (map-bundled) features — Stage 2.

## Step 6 — Critical pitfalls (read twice)

From phase-3-plan.md C2/B6 and the research digests:

1. **`teams[]` carries ONLY `startPos`, never `allyTeam`.** The engine
   consumes a flat pool; allyteam membership is purely UX scaffolding +
   `mapconfig/map_startboxes.lua`. Do NOT add an `ally_team` field to
   `TeamBlock` in the schema or emit it in the Lua.

2. **`mapconfig/featureplacer/features.lua` is at archive root,**
   NOT inside `LuaGaia/`. Don't confuse with `FeatureDefs` engine path.

3. **Lua string escaping.** `\` and `"` need backslash-escaping; newlines
   in `description` become `\n` not literal. Write unit tests covering
   `description = "Has \"quotes\" and\nnewlines"` → round-trips.

4. **NFR-Determinism**: re-running Build with no edits MUST produce
   byte-identical output across all four files. Sort `ally_groups` by
   `id` deterministically; sort feature lists (when C6 populates) by
   `(name, x, z)`.

5. **Pre-Phase-3 `.barmeproj` migration**: pre-existing projects with
   flat `start_positions` MUST load with all positions in
   `ally_groups[0]`. Write a fixture test. A silent data-loss bug here
   destroys user projects.

6. **`Color32::color_edit_button_srgba`** popup loses state across tool
   switches if the tree is rebuilt. Use a persistent `egui::Id::new`
   per AllyGroup (e.g. derived from `ally_group.id`).

7. **Drag-paint vs single-click**: B1 set `drag_threshold` to 8 px.
   Verify N=1 single-click doesn't fire drag-paint logic (would place
   a single point but the threshold guard prevents the drag path).

8. **Derived positions recompute every frame** — don't store them.
   Storing them risks drift when symmetry toggles. The trade-off:
   toggling symmetry off mid-session "forgets" the mirrored positions;
   that's acceptable and documented.

9. **`Project.ally_groups` order matters at emission time.** The flat
   `teams[]` list is `ally_groups[0].positions ++ ally_groups[1].positions ++ …`.
   If a user reorders ally groups, the resulting `teams[]` order changes
   — lobby slot assignments shift. Document this in the devlog.

10. **`box_polygon` coords are 0..1 fractions of map size**, not elmos.
    Convert at emission time. Standard 8v8 layout: ally 0 box =
    `[(0.0, 0.0), (1.0, 0.12)]` (north strip), ally 1 box =
    `[(0.0, 0.88), (1.0, 1.0)]` (south strip).

11. **No `allyTeams[]` field in mapinfo.lua.** Tempting to add one — DO
    NOT. The research is unambiguous: allyteam membership is lobby-side
    (script.txt) or `map_startboxes.lua` only.

## Step 7 — Exit criteria

- 3 commits on `main`: C2, B6 + rollup.
- 2 devlog folders filled.
- 2 checkboxes ticked in phase-3-plan.md.
- ADR-029 + ADR-032 in `docs/DECISIONS.md`. ADR-013 and ADR-023 status-updated.
- SRS § F8 + Project-model annotations updated.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Open pre-Phase-3 `.barmeproj` (assets/fixtures or a Phase-2-era
    project) → loads into `ally_groups[0]` with all prior positions.
  - Apply `8v8` preset → 16 positions across 2 ally groups, north/south
    strips, default colours blue/red.
  - LMB-drag across map in StartPositions tool → 8 positions distributed
    along vector in the active ally group.
  - Ctrl-Z undoes a single placement; Ctrl-Z again undoes another.
  - Hover Inspector row → marker pulses; hover marker → tree scrolls + highlights.
  - Build & Install → `.sd7` contains 4 Lua files at expected paths:
    `mapinfo.lua`, `mapconfig/map_metal_layout.lua` (empty body),
    `mapconfig/map_startboxes.lua` (populated for ≥2 ally groups),
    `mapconfig/featureplacer/features.lua` (empty body).
  - Load in BAR → 8v8 lobby option works; players spawn in their assigned
    ally group's strip.
  - Re-run Build with no edits → byte-identical `.sd7`.
- Final devlog log summarising what shipped + "Sprint 6 = C3 + B7 + B8
  (mapinfo defaults + procgen UX + demo state)" handoff note.

Start by running `git status`, then reading the files in Step 1. Begin with
C2 — the emitter contract drives B6's data shape.
