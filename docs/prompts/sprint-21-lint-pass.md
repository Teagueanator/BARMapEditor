# Sprint 21 — Lint My Map (C8)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 21** — **C8** (Lint My Map pass). After this sprint,
the editor produces `.sd7` files that pass a programmatic lint covering
every silent failure mode in PITFALLS.md §1–§21 (plus the audit
additions). Builds with hard errors are gated; warnings are surfaced
but non-blocking.

The lint UI surface (validation chip in the top-bar + per-tab dots
in the F9 form + lint panel Window) was scaffolded in **Sprint 18**
(F9 form) and **Sprint 19** (validation chip click + status-strip
issue count). This sprint fills the backing data with real issues
from a real `pub fn lint(project: &Project) -> Vec<LintIssue>` pass.

**Prerequisites:**
- Sprint 18 (minimap + F9 mapinfo form) MUST be ticked. The lint
  surfaces issues against the F9 form's per-tab dots.
- Sprint 19 (UI tooltip + help-text pass + validation chip click)
  MUST be ticked. The lint panel Window stub from Sprint 19 gets
  populated here.
- Sprint 20 (async build pipeline + log) MUST be ticked. The lint
  surfaces in the build-overlay too, listing top-3 errors when the
  Build button is clicked on an erroring project.
- Sprint 10 mapinfo audit fix MUST be present — half the lint rules
  catch regressions of audit-corrected fields.
- Sprints 11 + 12 + 13 + 14 + 15-17 establish the project model
  surfaces the linter walks (metal_spots, geo_vents, features,
  layers, water_overrides).

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §2.1 entire pitfall
   list, §3.2 F9 (the form C8 surfaces against), §3.3 NFRs (lint
   should run on project mutation, not per-frame).
3. **`/home/teague/code/BARMapEditor/docs/PITFALLS.md`** —
   EVERY rule. The lint pass is a 1:1 mapping of these rules into
   `LintRule` enum variants.
4. `/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`
   — §12 NEW-1 through NEW-10 (each NEW-N is a `LintRule`).
5. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/mapinfo.rs`
   — emitter (lint runs against the schema, post-emission for
   string-shape rules).
6. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — schema. Most lint rules read here.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/inspector_mapinfo.rs`
   (from Sprint 18 / C7) — where lint dots render.
8. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/lint_panel.rs`
   (from Sprint 19 / U1 — stub) — where lint issues render full.
9. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs::validation_summary`
   (function ~line 1676) — the old 8-state validation chip. This
   sprint REPLACES it with the new lint output; the chip itself
   stays.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-lint-pass
```

## Step 3 — Scope

One commit per logical chunk, then a rollup:

### 1. Lint module + types

**New module:** `crates/barme-pipeline/src/lint.rs`.

```rust
pub enum LintSeverity { Error, Warning, Info }

pub enum LintRule {
    // Hard errors — block build.
    ModtypeNotThree,                      // PITFALL §21
    DependMissingMapHelper,               // PITFALL §6 / SRS §1.3
    SmtFileNameZeroMissing,               // PITFALL §6 / "pink map"
    NameOrMapfileOrVersionMissing,        // PITFALL §6
    VoidWaterWithPlaneColor,              // PITFALL §6
    TeamsEmpty,                           // PITFALL §6 / SRS §1.3
    FeatureNotInStockManifest,            // PITFALL §6 (Stage 2: bundled)
    SplatDetailNormalTexWithoutSpecular,  // PITFALL §6 + §17 reworded
                                          //   (FINDINGS §7.2: "still renders, looks flatter")
    FogStartEqualsFogEnd,                 // PITFALL §6
    HeightmapDimsWrong,                   // PITFALL §4

    // Warnings — surface but don't block.
    LightingSunDirMissing,                // PITFALL §11
    LightingSunDirLowercaseOnly,          // PITFALL §11 (inverse: emit BOTH)
    AtmosphereSkyDirPresent,              // PITFALL §12 (deprecated)
    GuiMinimapRotationPresent,            // PITFALL §19 (unused)
    ExtractorRadiusFiveHundred,           // PITFALL §6 (engine default,
                                          //   BAR uses 80)
    TidalStrengthWithoutWaterSurfaceColor,// PITFALL §6
    TerrainBelowZeroWithoutWater,         // PITFALL §6 — Sprint 14
                                          //   surfaces this live in the
                                          //   validation chip; C8 promotes
                                          //   to the lint panel.
    WaterModeSetWithoutTerrainBelowZero,  // Sprint 14 / C9 — inverse.
                                          //   Fix offers two options: enable
                                          //   `forceRendering = true` OR
                                          //   carve `min_height < 0` via the
                                          //   Water tool's flood brush.
    TeamsLessThanSixteenOnLargeMap,       // PITFALL §6
    StartboxesLuaMissingWhenMultiTeam,    // PITFALL §6
    ResourcesDetailTexMissingOnDntsMap,   // PITFALL §6
    GeoInMetalLayoutGeosArray,            // PITFALL §14 — should never fire,
                                          //   but guard for user-imported
                                          //   projects that carry it.
    SmfMetalmapNonZeroWithLuaSpots,       // PITFALL §13
    SunDirWIsLarge,                       // PITFALL §18 (w > 100; 1e9 leakage)
    SplatDetailNormalTexLegacyForm,       // PITFALL §15 (encourages subtable)
    DntsOnMapWithMinHeightBelowZero,      // PITFALL §8

    // Info — convention notes.
    GravityNotOneThirty,                  // PITFALL §6 (BAR convention)
    ExtractorRadiusDriftFromEighty,       // PITFALL §6 (BAR convention)
    VoidGroundWithoutVoidAlphaMinTuning,  // PITFALL §20 (cosmetic)

    // New audit additions (PITFALLS §22+ from Sprint 11/14 hotfixes).
    StartPositionShapeWrong,              // PITFALL §23 — start pos must wrap
                                          //   in `{}` not bare key/value
    LuaGaiaTeamMissing,                   // PITFALL §24 — every multiplayer
                                          //   map needs the LuaGaia bootstrap
                                          //   pair (verify via emitted teams)
    MetalValueOutOfBARRange,              // PITFALL §25 — yield scale check
}

pub struct LintIssue {
    pub rule: LintRule,
    pub severity: LintSeverity,
    pub message: String,
    pub field_path: Option<String>,  // e.g. "lighting.sun_dir.w"
    pub fix: Option<LintFix>,        // closure-shaped action
}

pub enum LintFix {
    SetField(String, toml::Value),   // for simple "set X to Y" fixes
    ApplyDiff(ProjectDiff),          // for compound mutations
}

pub fn lint(project: &Project) -> Vec<LintIssue>;
```

### 2. Rule implementations — hard errors first

One commit per severity tier. Each rule is a function:

```rust
fn lint_modtype(info: &MapInfo, out: &mut Vec<LintIssue>) {
    if info.modtype != 3 {
        out.push(LintIssue {
            rule: LintRule::ModtypeNotThree,
            severity: LintSeverity::Error,
            message: format!(
                "modtype must be 3 (map). Found {}. \
                 Per PITFALL §21: 0=hidden, 1=primary, 2=unused, \
                 3=map, 4=base, 5=menu.",
                info.modtype,
            ),
            field_path: Some("modtype".into()),
            fix: Some(LintFix::SetField(
                "modtype".into(),
                toml::Value::Integer(3),
            )),
        });
    }
}
```

Hard-error rules first (one commit), then warnings (one commit),
then info-level (one commit). Test each rule with a fixture
project that DOES and DOES NOT trigger it.

### 3. UI wiring — populate Sprint 19's stub

`crates/barme-app/src/main.rs` + `crates/barme-app/src/ui/lint_panel.rs`
(extends Sprint 19's stub):

- **Debounced lint trigger**: `App::lint_summary: Option<Vec<LintIssue>>`,
  recomputed on `Project` mutation via a 250 ms debounce. Cached
  across frames; only re-runs on project diff.
- **Status-strip chip**: the existing validation-chip from Sprint 19
  lights up with the issue count + severity. Click → opens the lint
  panel (already wired in Sprint 19).
- **Lint panel** (populates the Sprint 19 stub):
  - Header: total count by severity.
  - Group by severity (Errors first).
  - Each row: rule name, message, field_path (if set), "Fix" button
    (if `fix.is_some()`).
  - Clicking Fix applies the diff via the existing
    `ProjectDiff` machinery (undo-able per B5).
  - Each row's rule name is clickable → opens the PITFALLS.md
    URL in the user's browser (or shows the pitfall text inline
    via Sprint 22's help center if that's shipped first).
- **F9 form dot integration** (from Sprint 18 / C7): the per-tab
  dots already wired in Sprint 18 read `App::lint_summary`. Match
  by `field_path` prefix (`lighting.*` → Lighting tab).
- **Build gating**: Errors block the Build button (greyed). The
  user can still Save / Export Raw Lua / inspect; just can't
  publish a broken `.sd7`. The Build button's
  `on_disabled_hover_text` explains why (lists top-3 errors).

### 4. Build-overlay integration (touches Sprint 20)

When the user clicks the (errored) Build button:
- If `lint_summary` has ≥1 Error: show a small dialog summarising
  the errors with a "Show in lint panel" button. Don't run the
  build.
- If only warnings: build proceeds; the build overlay's header
  surfaces "Building with N warnings — see lint panel".

### 5. Tests

- **Unit test per rule**: positive (rule fires) + negative (rule
  silent). 30+ rules × 2 cases = 60+ tests minimum. Use a
  shared fixture-builder helper (`tests::common::fixture_project`).
- **Integration test**: load a freshly-wizard-created project →
  `lint(&project)` returns 0 errors, ≤ 2 warnings. (Default
  project should be lint-clean by construction.)
- **Integration test**: deliberately corrupt `modtype = 0` →
  `lint` returns exactly 1 error of `LintRule::ModtypeNotThree`.
- **UI snapshot test**: lint panel renders correctly when given
  a fixture `Vec<LintIssue>` (egui smoke test pattern).
- **Build-gating test**: assert `build_and_install` (Sprint 20's
  worker) refuses to execute when `lint_summary` has ≥1 Error.

### 6. Rollup commit

- STATUS UPDATEs in SRS / ROADMAP (C8 ticked).
- closing devlog log.
- **Stage 1 internal-work-complete announcement** in the devlog —
  every F-item except F12 (deferred to Sprint 32) is now shipped.
- "Sprint 22 = Onboarding + contextual help + command palette"
  handoff note.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on lint pass start/end
with summary; `trace!` on per-rule fires; `warn!` on lint-panel
errors.

## Step 5 — Out of scope

- **F12 Launch in BAR** — Sprint 32.
- **F13 decompile / import** — Stage 2.
- **F14 procgen v2** (FBM / hydraulic erosion / river carve) —
  Stage 2.
- **C8's "auto-fix all warnings" bulk operation** — defer;
  risky to apply without review.
- **PITFALLS §26-28 lint rules** (look_at_lh sign-flip,
  GetWaterPlaneLevel consteval, min_height shader plumbing) —
  these are renderer-side pitfalls already enforced by the
  Sprint 14 / Sprint 13 codepath; not lint-surfaceable from
  project state alone.

## Step 6 — Critical pitfalls (read twice)

1. **Lint runs on mutation, NOT per-frame**. Debounce 250 ms after
   the last project mutation. A 16-SMU map with 64 start positions
   + 32 metal spots is still <10k items to walk; the lint should
   complete in <5 ms.

2. **Fix actions go through `ProjectDiff` / undo**. The user must
   be able to Ctrl-Z a Fix click. Don't mutate the project
   directly; emit a diff.

3. **Build button gating**: gate ONLY on errors, NOT warnings.
   Warnings should be visible but not blocking — the user might
   intentionally ship a 12-SMU empty map for a test.

4. **The `SplatDetailNormalTexWithoutSpecular` rule's wording**
   (FINDINGS §7.2): NOT "DNTS silently disables." Recoil's current
   render-state gates DNTS on `splatDistrTex && splatDetailNormalTex[]`
   only. Reword:

   > "No specular texture set. DNTS still renders, but the
   > result looks noticeably flatter than published BAR maps.
   > Sprint 12 / D6 ships a default grey specular when none is
   > set — this lint warns if both are missing."

5. **`HeightmapDimsWrong` rule**: heightmap dims must be
   `(64 * N + 1)²` (PITFALL §4). The wizard prevents this from
   happening; the lint guards against external import
   (F13 / Stage 2).

6. **`GeoInMetalLayoutGeosArray` rule** should NEVER fire from a
   project the editor created — Sprint 11 / C5 emits geos only
   as features. But guard against user-imported projects that
   carry the Zero-K convention; surface as a warning + offer a
   fix that converts the `geos` array into `geovent` feature
   placements.

7. **`StartboxesLuaMissingWhenMultiTeam`**: if
   `Project.ally_groups.len() > 2`, `mapconfig/map_startboxes.lua`
   should be non-empty. Sprint 5 / C2 emits it from
   `Project.ally_groups[*].box_polygon`. Lint fires if the user
   has multiple ally groups but no `box_polygon` set on any of
   them.

8. **`FeatureNotInStockManifest` rule**: the manifest is at
   `assets/mapfeatures_catalog.json` (committed in Sprint 12 / C6).
   Map-bundled features are Stage 2; in Sprint 21 the rule fires
   on ANY unknown feature name. Surface a fix that opens the
   feature inspector for the user to delete the bad feature.

9. **Test coverage**: every rule has a positive AND negative test.
   Without the negative, regressions can flip a rule's predicate
   without test failures. Don't skip the boring "rule doesn't fire
   on clean input" case.

10. **NFR-Performance**: the lint must NOT block the UI thread on
    a large project (32 SMU + 500 features). Run lint via
    `std::thread::spawn` if it ever exceeds 16 ms; the App reads
    the result through a `crossbeam::channel`. For Sprint 21 keep
    it synchronous + debounced; add the threading later if perf
    bites.

11. **Sprint 19's stub `LintPanel` populates incrementally**. Don't
    rewrite it; extend. The stub already opens on chip click; this
    sprint just feeds the body real data.

12. **Build-gating regression**: ensure that disabling the Build
    button does NOT also block Save / Save As / Export Raw Lua /
    inspector edits. Only the Build action is gated. Test the
    File menu items remain enabled when lint has errors.

## Step 7 — Exit criteria

- ~5 commits on `main`: types, hard-error rules, warning rules,
  info rules, UI wiring + rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs ("Lint My Map shipped; Stage 1
  internal work complete except F12").
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- ≥30 `LintRule` variants implemented with positive + negative tests.
- Smoke test (record in final devlog log):
  - Default wizard project → 0 errors, ≤ 2 warnings in the chip.
  - Deliberately set `modtype = 0` → 1 red error chip. Click →
    panel shows `ModtypeNotThree` with a "Set modtype = 3" Fix
    button. Click Fix → modtype = 3, undo restores.
  - Deliberately set `extractor_radius = 500` → 1 yellow warning.
  - Build button greyed when ≥1 error; click greyed button →
    tooltip lists top-3 errors. Save / Save As / Export Raw Lua
    remain enabled.
  - Open F9 form (Sprint 18 / C7) → tab dots light on tabs with
    issues; clicking a dot scrolls to the field.
- Final devlog log:
  - Summary of what shipped this sprint.
  - **Stage 1 internal-work milestone announcement** —
    F1–F11 + F8 + F14 (math subset) + F15 (terrain types data
    only) + the editor maturity work all landed. F12 (Launch
    in BAR) remains for Sprint 32, gated on the build pipeline
    from Sprint 20.
  - Pivot note: Sprint 22 = onboarding + contextual help +
    command palette (third and final UI polish sprint).

Start by reading PITFALLS.md end-to-end one more time — every rule
in the lint maps to a numbered pitfall. The lint module is mostly
mechanical once the rule list is fixed; the bulk of the work is
test coverage and the UI panel.
