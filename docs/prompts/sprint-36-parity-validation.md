# Sprint 36 — Parity validation + SRS §2.1 #11 closeout (R8)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 36** — the **final renderer-parity sprint** (R8 in
ROADMAP numbering). After this sprint, SRS §2.1 #11 flips from
"3D preview ≠ in-game rendering. Do not pretend WYSIWYG" to "3D
preview reproduces BAR's render at editor camera distances within
mean ΔE 5.0 across the validation suite. See Sprint 36
validation report."

The sprint is **automation + acceptance**, not feature work. The
goal: an automated ΔE harness that compares editor screenshots
against BAR screenshots across 3 reference maps at 3 camera
angles each (9 comparisons total). The harness runs in CI and
produces a drift report.

**Prerequisites:**
- Sprint 35 (emission + sky-reflect + parallax) MUST be ticked
  — the last shader feature lands first.
- Sprint 33 (NFR/CI gates) MUST be ticked — CI infrastructure
  is in place.
- Sprint 18 (minimap + F9 form) MUST be ticked — headless
  render pipeline is the harness foundation.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §2.1 #11 (the
   commitment).
3. `/home/teague/code/BARMapEditor/docs/research/renderer-bar-parity/ROADMAP.md`
   — Sprint 23 (original numbering) = this Sprint 36.
4. Existing parity fixtures (Sprints 25, 26, 28, 30, 34, 35) at
   `assets/parity-fixtures/` — these are the reference targets.
5. Sprint 18's `headless_render.rs` — the harness extends it.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-36-parity-validation
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. ΔE (Delta-E) computation

`crates/barme-app/src/delta_e.rs` (new) or new crate
`barme-validate`:

```rust
pub struct DeltaEReport {
    pub mean: f32,
    pub max: f32,
    pub percentile_95: f32,
    pub pixels_above_5: usize,
    pub total_pixels: usize,
}

pub fn compute_delta_e(
    image_a: &image::RgbImage,
    image_b: &image::RgbImage,
) -> DeltaEReport;
```

Use the CIE2000 ΔE formula (or CIE76 for simplicity if precision
isn't critical). Add `palette` crate or implement manually.

ΔE < 5 is "indistinguishable" perceptually.

### 2. Reference BAR screenshot capture

Manual step (documented in devlog):
- For each of 3 reference maps (Comet Catcher Remake, All That
  Simmers, a lava/water sample):
  - Load in BAR via `--isolation` mode.
  - Render at top-down, 35° pitch, grazing camera angles.
  - Screenshot to `assets/parity-fixtures/<map>/bar-reference/<angle>.png`.
- Pin the BAR version (commit hash from `RecoilEngine`).

These screenshots are committed to the repo (~3 × 9 = 27 PNGs at
512² = ~5 MB). Don't grow further; this is the validation
baseline.

### 3. Editor screenshot capture

`crates/barme-app/tests/parity_validation.rs`:

```rust
#[test]
fn editor_matches_bar_within_delta_e_5() {
    for map in &["comet", "simmers", "lava"] {
        for angle in &["top", "35deg", "grazing"] {
            let project = load_parity_fixture(map);
            let editor_screenshot = headless_render(&project, angle).unwrap();
            let bar_reference = image::open(format!(
                "assets/parity-fixtures/{}/bar-reference/{}.png",
                map, angle
            )).unwrap();

            let report = compute_delta_e(&editor_screenshot, &bar_reference.to_rgb8());
            assert!(
                report.mean < 5.0,
                "{}/{}: mean ΔE = {:.2} (>5.0)",
                map, angle, report.mean
            );
        }
    }
}
```

### 4. Drift report

Run the harness; produce a `docs/parity-drift-report.md` listing
every comparison's ΔE numbers. Items where ΔE > 5 get either:
- A polish task on the renderer arc backlog.
- A documented "we don't render X because Y" note.

### 5. SRS update

Edit `/home/teague/code/BARMapEditor/SRS.md` §2.1 #11:

```diff
-#11. 3D preview ≠ in-game rendering. Do NOT pretend WYSIWYG.
-     The editor's preview is approximate; users must build + launch
-     to see the truth.
+#11. 3D preview reproduces BAR's render at editor camera distances
+     within mean ΔE 5.0 across the 9-case validation suite.
+     See `docs/parity-drift-report.md` for the per-case report.
+     Closed by Sprint 36 / R8 on 2026-05-XX.
```

### 6. CI integration

The parity test runs on the slow lane (gated by a feature flag
since it needs ~30s for the 9 renders). Failures upload the
editor vs BAR screenshots side-by-side as artifacts for triage.

### 7. ADR-045 + rollup

```
## ADR-045 — Renderer-parity validation closeout (Sprint 36 / R8)

Status: ADOPTED 2026-05-XX
Supersedes: SRS §2.1 #11 (original wording).
...
```

STATUS UPDATEs in SRS / ROADMAP (R8 done; renderer-parity arc
COMPLETE 8/8). "Stage 1 + renderer parity DONE; external
Beherith review next" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on each ΔE comparison
with result; `warn!` on >5 ΔE; `error!` on harness failures.

## Step 5 — Out of scope

- **Re-rendering BAR reference shots** — manual capture only.
- **Per-pixel diff visualisation as an editor feature** —
  test-only.
- **HDR comparison** — sRGB only.
- **Animation parity** (wind / waves / grass over time) —
  static frame only.
- **Multi-GPU comparison** — single-GPU baseline.

## Step 6 — Critical pitfalls (read twice)

1. **BAR version pin**: reference screenshots are tied to a
   specific BAR commit. If BAR's renderer changes, the
   screenshots may stale. Document the pin; review on every
   BAR major release.

2. **GPU driver determinism**: rendering on different drivers
   (Vulkan vs GL vs Metal) yields slightly different pixels.
   The ΔE threshold of 5 absorbs this. If a CI runner produces
   ΔE = 10 vs the human-eye-OK 3, the harness is too sensitive
   — relax to 8 or 10 on CI, keep 5 for local dev.

3. **Time-dependent shaders**: water + grass + caustics
   animate. The reference shot captures a fixed `time = 0`. The
   editor's headless render must also set `time = 0` for parity.

4. **Camera angles**: top-down + 35° + grazing must be **exactly
   reproducible**. Use fixed Euler angles + map-AABB framing.
   Don't rely on orbit-state preservation.

5. **Resolution**: reference shots at 512²; editor shots at 512²
   (downsample if rendering larger). Same DPI.

6. **Light state**: BAR has a sun-direction default if mapinfo
   omits. The editor (Sprint 6 / C3) seeds BAR-default lighting.
   Cross-check; ensure parity defaults match.

7. **Validation as gate vs report**: Sprint 36 produces a report,
   not a gate that blocks future PRs. Future renderer changes
   should regenerate the report manually and review the diff.
   CI gate would over-rotate on driver noise.

8. **Skybox / cubemap absent in BAR reference**: if a reference
   map doesn't set `skyBox`, BAR uses a default sky shader. The
   editor (Sprint 28) falls back to `sky_color`. Both must
   match.

9. **Headless on macOS / Windows**: the harness runs on Linux
   primary; macOS / Windows CI is best-effort. Document the
   matrix.

10. **The drift report is a living document**: future renderer
    sprints (Stage 2 polish) iterate on items where ΔE > 5.
    The report's `Open items` section is the renderer-arc
    backlog.

## Step 7 — Exit criteria

- 4+ commits on `main`: ΔE harness, reference + editor capture,
  drift report, SRS update + ADR-045 + rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (R8 done; renderer-parity arc
  COMPLETE 8/8; SRS §2.1 #11 closed).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green (with parity test marked slow lane).
- Smoke test:
  - 9-case parity test runs locally; mean ΔE < 5 for at least
    7 of 9. Drift report lists the failing cases.
  - Reference + editor screenshots side-by-side render
    indistinguishable to the human eye at 2-8 SMU camera
    distance.
- Final devlog: summary + Stage 1 + renderer-parity arc
  COMPLETE announcement + "Sprint 37 = brushes + symmetry
  line picker" handoff.

Start by capturing the BAR reference shots manually (use BAR's
in-game F12 screenshot key with `gui_hide` for clean output).
Then implement ΔE. The harness is mechanical once the data is
in place.
