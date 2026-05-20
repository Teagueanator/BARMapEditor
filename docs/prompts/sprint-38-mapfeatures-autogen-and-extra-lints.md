# Sprint 38 — Mapfeatures catalog auto-generation + PITFALLS §22-28 lint rules (L2)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 38** — a small cleanup sprint with two complementary
threads:

1. **Mapfeatures catalog auto-generation** — the hand-curated 30-
   entry `assets/mapfeatures_catalog.json` was promised in Sprint
   12 / C6 as "polish task" but never scheduled. Auto-generate from
   the upstream `github.com/beyond-all-reason/mapfeatures` repo's
   Lua definitions so the catalog stays in sync.

2. **Additional lint rules for PITFALLS §22-28** — Sprint 21 (C8)
   shipped lint rules for §1-21. The post-Sprint-11/14 pitfalls
   (§22-28) — start-position shape wrapping, LuaGaia bootstrap,
   metal yield scale, look_at_lh sign-flip, GetWaterPlaneLevel
   consteval, min_height shader plumbing — are partially covered.
   Sprint 38 closes the gap.

**Prerequisites:**
- Sprint 37 (brushes + symmetry line) — F2 + F3 fully closed.
- Sprint 21 (lint pass C8) — lint module exists and is extensible.
- Sprint 29 (feature decoding) — confirms catalog entries' visual
  representation.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §22-28
   (post-audit additions; some already test-covered, not all lint-
   gated).
3. `/home/teague/code/BARMapEditor/assets/mapfeatures_catalog.json`
   — current hand-curated state.
4. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/lint.rs`
   (Sprint 21) — extend with new rules.
5. Upstream mapfeatures repo (clone to `~/code/Beyond-All-Reason/mapfeatures`).

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-38-mapfeatures-autogen
./devlog/log.sh new sprint-38-pitfalls-22-28-lints
```

## Step 3 — Scope

### 1. Mapfeatures catalog autogen tool

`crates/barme-tools/src/bin/regen_mapfeatures.rs` (new bin, in a
new dev-only crate `barme-tools` if not present):

```rust
// Parse upstream `features/*.lua` files (mlua or hand-rolled Lua-table parser).
// Emit `assets/mapfeatures_catalog.json` with: name, display, category,
//   metal, s3o_path, tags.
// Committed to repo; regenerated only on demand (not in CI).
```

Output schema matches the existing `mapfeatures_catalog.json`. The
script preserves the manually-curated `display` and `tags` fields
for entries already present; new entries get auto-generated defaults
with `TODO` markers.

**Trigger**: `cargo run -p barme-tools --bin regen_mapfeatures --
--mapfeatures-root ~/code/Beyond-All-Reason/mapfeatures`. Not
automatic; the user runs after a `mapfeatures` upstream update.

### 2. PITFALLS §22-28 lint rules

Audit which §22-28 pitfalls are NOT yet lint-gated and add rules:

- **§22** (review what this covers; may already be in Sprint 21).
- **§23 StartPositionShapeWrong** — Sprint 21 added this rule.
  Verify it fires correctly with the new schema.
- **§24 LuaGaiaTeamMissing** — Sprint 21 added it. Verify.
- **§25 MetalValueOutOfBARRange** — Sprint 21 added it. Tighten
  the range to BAR conventions (0.5 / 2.0 / 4.0 / 5.2 standard
  values; warn if metric drifts).
- **§26 LookAtLhSignFlip** — renderer-side pitfall. Sprint 14
  fixed it (PITFALL note in `viewport.rs`). Lint here as a
  defensive "if you ever import a project with manual axis values,
  warn on Z<0 inversions". Low priority.
- **§27 LookAtLh** + **§28 GetWaterPlaneLevel consteval** — both
  are renderer-side, already enforced by the codepath; surface as
  Info-severity reminders in the lint panel for documentation
  purposes (no actual error condition possible from project state).

### 3. Tests + rollup

- **Catalog regen test**: re-run regen against a checkpointed
  upstream snapshot; assert output is byte-identical to the
  committed JSON.
- **Lint rule positive/negative tests** for each new rule.
- **Rollup commit**: STATUS UPDATEs in SRS / ROADMAP (L2 done +
  Sprint 12 / C6 polish task closed). closing devlog logs.
  "Sprint 39 = F23 user-asset library" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on catalog regen run;
`trace!` on per-rule fires.

## Step 5 — Out of scope

- **F-asset-library** (F23) — Sprint 39.
- **mlua dep**: prefer a lightweight Lua-table parser; avoid
  pulling in full mlua just for `features/*.lua`.
- **CI auto-update of catalog**: the regen runs on-demand by the
  user, not by CI. Upstream `mapfeatures` may break trust.

## Step 6 — Critical pitfalls

1. **Catalog regen overwrites manual curation**: the `display`
   + `tags` fields are user-authored. The script preserves them
   for known entries; flag this in the commit message.

2. **Upstream schema drift**: `mapfeatures` repo may change Lua
   key names. The script should fail fast with a clear error
   ("expected key X in features/Y.lua, found nothing") and the
   user manually patches.

3. **Lint rule coverage gaps for §22-28**: some pitfalls aren't
   lint-surfaceable from project state (e.g., §28 is a `consteval`
   issue in the engine source). Document as Info-severity or
   skip with a `// not lint-surfaceable` annotation.

## Step 7 — Exit criteria

- 4+ commits on `main`: catalog regen tool, regen + commit the
  freshly-generated catalog (no diff if upstream unchanged),
  PITFALLS §22-28 lint rules + tests, rollup.
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (L2 + Sprint 12 / C6 polish task).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: catalog regen → commit. Lint panel shows §22-28
  rule rows when triggered by fixture projects.
- Final devlog: summary + "Sprint 39 = F23 user-asset library"
  handoff.
