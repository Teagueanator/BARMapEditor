# Sprint 3 — Symmetry global + canvas affordances + top-bar build (B2, B3, B4)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 3** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship **B2, B3, B4** — three small UX items that polish the
layout shell from Sprint 2.

**Prerequisites:** Sprint 1 (A1–A4) and Sprint 2 (B1) should already be
ticked in phase-3-plan.md, and ADR-030 (layout shell) should be in
`docs/DECISIONS.md`. Verify both before starting. If B1 is unticked, stop —
finish B1 first; this sprint assumes the five-zone shell exists.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.2 (functional reqs).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable rules.
4. `/home/teague/code/BARMapEditor/devlog/README.md`.
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md`.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` — read
   B2, B3, B4 in full, plus "Standing constraints" and "Note on devlog
   discipline."
7. `/home/teague/code/BARMapEditor/devlog/stage-1-ux-layout-shell/logs/`
   — read the most recent log so you understand what B1 left for you.
8. `/home/teague/code/BARMapEditor/docs/research/ui/claude-research-findings.md`
   — skim the "Symmetry as a global mode," "Brush ring + on-canvas feedback,"
   and "Camera affordances" sections.
9. ADR-019 in `docs/DECISIONS.md` — symmetry data model (you're moving the
   UI surface, not the data).
10. ADR-030 — layout shell you're decorating.
11. `crates/barme-app/src/main.rs` and `crates/barme-app/src/ui/` — the
    five-zone shell B1 produced.
12. `crates/barme-app/src/render.rs` — `world_to_screen` (added in ADR-023)
    is what B2 and B3 use for the canvas overlays.

## Step 2 — Devlog flow (per item)

Three feature folders, one per item:

```bash
./devlog/log.sh new stage-1-ux-symmetry-global
./devlog/log.sh new stage-1-ux-canvas-affordances
./devlog/log.sh new stage-1-ux-topbar-build
```

Fill each folder from phase-3-plan.md (Scope → goals.md, Pitfalls → notes.md,
Hypothesis if present → theories.md). Add session logs as you go.

## Step 3 — Scope

In order, one commit per item:

1. **B2 — Symmetry as global mode + canvas axis overlay [ADR-031]**
   - Promote symmetry from the Sculpt-section radio into a top-bar chip.
     Chip text: `Sym: Off / Quad / Rot×4 / H / V / Diag` etc. Click → opens
     `egui::Window` popover with the existing controls (axis enum +
     rotational fold spinbox).
   - Persistent canvas overlay: when symmetry ≠ None, draw dashed axis
     lines through map centre via `ui.painter().line_segment` on the
     central viewport rect.
   - Mirror-brush ghosts: when Sculpt tool active AND symmetry ≠ None,
     render faint ghost brush rings at each symmetry image (50% alpha).
     B3 owns the primary brush ring; B2 just adds the ghosts.
   - Writes ADR-031.

2. **B3 — Brush ring + nav gizmo + first-launch hint + `?` cheat-sheet**
   - Brush ring: 2-ring circle at cursor world position via
     `ui.painter().circle_stroke`. Outer = radius, inner = radius × 0.5
     (falloff visual). Centre dot. Colour by brush (Raise green, Lower
     red, Smooth blue). Only when Sculpt tool active.
   - Nav gizmo: top-right corner of viewport (~80 px square). Painted
     axis compass; click-axis snaps camera to orthographic view; drag
     orbits.
   - First-launch hint: `egui::Window` overlay at app start (post-wizard)
     with three bullets ("LMB sculpt / RMB orbit / ? for help"). Dismiss
     writes `seen_intro: true` to a per-user config TOML keyed by editor
     version. New module `crates/barme-app/src/config.rs` for the TOML
     helper.
   - `?`-key cheat-sheet: modal Window listing keymaps **auto-generated
     from the tool enum + camera bindings** (no static markdown).
   - No ADR (small composite).

3. **B4 — Top-bar Build button + bottom status strip wiring**
   - Move Build & Install from side panel into top action bar (right edge).
     Primary-button styling (green via `Visuals::widgets::active.bg_fill`).
   - Adjacent `ComboBox` for variants: `Build`, `Build + Install`,
     `Build + Install + Launch` (last greyed pre-F12).
   - Bottom status strip wires to live data: camera position (1-Hz
     refresh — use `ctx.request_repaint_after(Duration::from_secs(1))`),
     map dims, validation chip count placeholder ("0 issues" until C8).
   - No ADR.

Then a **4th rollup commit**: STATUS UPDATEs in SRS / ROADMAP, tick 3 boxes
in phase-3-plan.md, closing devlog log.

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells.
- Before every commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects.
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing convention: `error!`/`warn!`/`info!`/`trace!`, `{e:#}`.
- Devlog folder per item.

## Step 5 — Out of scope

- B5, B6, B7, B8 — separate sprints.
- C / D / E streams.
- Depth-conformant wgpu-decal brush ring (Gemini's variant) — Phase 4 polish.
- Aseprite-style movable symmetry-axis handles — the engine assumes
  geometric-center symmetry (ADR-019 lore).
- Wiring the validation chips to a real lint pass — that's C8.
- Wiring "Launch in BAR" — that's F12 / Phase 5.

## Step 6 — Critical pitfalls (read twice)

From phase-3-plan.md:

1. **Dashed-line aliasing at small screen size.** `Painter::line_segment`
   dashed pattern can alias to a solid line. Compute dash spacing in
   *screen pixels* (e.g. 8 on / 4 off), not world units.

2. **Rotational symmetry with high fold (10, 12) crowds the centre.**
   Either fall back to a single thin circle inside the inner 20% of the
   map, or accept the crowding. Document the choice.

3. **Cheat-sheet drift.** The `?` modal must auto-generate from the tool
   enum + camera bindings. A static markdown cheat-sheet drifts; don't
   write one.

4. **First-launch flag location.** Per-user config TOML keyed by editor
   version (so a major release replays the hint once). NOT in the
   `.barmeproj` file.

5. **Brush ring world position.** Reuse the existing y=0 raycast from
   stamp placement. Don't add a second projection path.

6. **Build button colour.** egui's primary-button colour comes from
   `Visuals::widgets::active.bg_fill`. Don't hardcode an RGB — go through
   the visuals so dark/light theme support (F21) is preserved.

## Step 7 — Exit criteria

- 4 commits on `main`: B2, B3, B4 + rollup.
- 3 devlog folders filled.
- 3 checkboxes ticked in phase-3-plan.md.
- ADR-031 (symmetry global) in `docs/DECISIONS.md`.
- SRS / ROADMAP STATUS UPDATEs.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Symmetry chip in top bar; click opens popover with controls.
  - Quad symmetry shows two perpendicular dashed lines through map centre.
  - Sculpt tool active + Horizontal symmetry → primary brush ring at cursor
    + faint ghost ring on the mirror side.
  - Nav gizmo visible top-right; clicking +X snaps camera.
  - Fresh user-config → first-launch overlay appears; dismiss persists across restart.
  - `?` opens cheat-sheet listing Q/B/S/G + camera bindings.
  - Top-bar Build button green; combo offers 3 variants (Launch greyed).
  - Bottom status strip shows live camera coords (updates ~1 Hz) + map dims.
- Final devlog log summarising what shipped + "Sprint 4 = B5+C1 (undo
  for non-heightmap state + mapinfo schema model)" handoff note.

Start by running `git status` and reading the files in Step 1. Then begin B2.
