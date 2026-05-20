# Sprint 2 — Five-zone layout shell + tool-mode left strip (B1)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 2** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship one item — **B1**, the five-zone layout shell + tool-mode
left strip. B1 is a load-bearing UI refactor that touches every UI surface,
so it gets its own session.

**Prerequisites:** Sprint 1 (items A1–A4) should already be ticked in
phase-3-plan.md. Verify before starting; if any of A1–A4 is unticked,
finish that first (it's blocking nothing in B1 directly, but having a
clean Stream A baseline keeps the diff legible).

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.2 (functional reqs), §3.4
   (architecture).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable rules.
4. `/home/teague/code/BARMapEditor/devlog/README.md` — devlog system.
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md` — shipped/queued.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` — read
   the full **Stream B section** (B1 in detail, plus B2–B8 so you understand
   what your shell needs to support), plus "Standing constraints" and "Note
   on devlog discipline."
7. `/home/teague/code/BARMapEditor/docs/research/ui/claude-research-findings.md`
   — the UX research that justifies the 5-zone layout. Skim the
   "Layout Architecture" and "Tool-mode" sections.
8. Glance at `docs/research/ui/Gemini UX Redesign for BAR Map Editor.md`
   — for the divergence on single-left-strip vs two-left-panel. **We adopt
   Claude's single-strip stance** (see phase-3-plan.md B1 Hypothesis).
9. `crates/barme-app/src/main.rs` — the current monolithic `update()`. This
   is what gets restructured.
10. ADRs 017–024 in `docs/DECISIONS.md` for context on how prior UI was
    structured.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-ux-layout-shell
./devlog/log.sh log stage-1-ux-layout-shell "starting"
```

Fill `stage-1-ux-layout-shell/`:
- `goals.md` — from B1's Scope + Success criteria in phase-3-plan.md.
- `theories.md` — from B1's Hypothesis (single-strip > two-strip).
- `notes.md` — live design sketches, especially around how
  tool-specific Inspector state lives on `App` / `Project` (not
  `ui.memory()` — that's the "immediate-mode state ownership" pitfall).
- `logs/<timestamp>__<title>.md` — session logs.

## Step 3 — Scope

Ship **B1 only**. This is one large commit + a rollup commit.

**B1 deliverable:**
- Restructure `App::update` into five zones in egui panel add-order:
  - `TopBottomPanel::top` (action bar with File / Edit menus + symmetry chip
    placeholder + Build & Install)
  - `TopBottomPanel::bottom` (status strip — live camera coords, map dims,
    validation chip placeholders)
  - `SidePanel::left` (40 px fixed, vertical tool icon strip)
  - `SidePanel::right` (300 px resizable, Inspector)
  - `CentralPanel::default()` (last — the wgpu viewport)
- Single-active-tool enum: `Tool { Select, Sculpt, StartPositions, Procgen }`.
  Phase 4 will add Splat / Metal / Feature variants — leave room.
- One-letter accelerators: `Q` Select, `B` Brush/Sculpt, `S` StartPositions,
  `G` Generate/Procgen. Tool change emits `tracing::info!`.
- Inspector contents driven by exhaustive `match` on the tool enum.
- Persistent block at Inspector top (always visible regardless of tool):
  project name, map size (`smu_x × smu_z`), heightmap dims, max height.
- Migrate existing controls into their new homes:
  - Symmetry → top-bar chip (B2 will wire the popover + canvas overlay; for
    this commit, clicking the chip can open a placeholder `egui::Window` or
    just be visually present).
  - Brush radius/strength → Inspector (Sculpt tool active).
  - Procgen presets + expression → Inspector (Procgen tool active).
  - Start-position list / placement params → Inspector (StartPositions
    tool active).
  - Build & Install button → top-bar right edge (B4 styles it green +
    adds the variants dropdown; for this commit, plain Button is fine).
- `ctx.set_drag_threshold(Vec2::splat(8.0))` to disambiguate
  click-place from drag-paint (per phase-3-plan.md pitfall).
- New module dir `crates/barme-app/src/ui/` if helpful, with one file per
  zone. Otherwise keep inline — judgement call.

**Out of scope** (later sprints):
- Symmetry canvas overlay (B2).
- Brush ring + nav gizmo (B3).
- Top-bar Build button styling + variants combo (B4).
- Splat / Metal / Feature tool variants (gated on D5 / C4 / C6).

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells.
- Before every commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects.
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing convention: `error!`/`warn!`/`info!`/`trace!`, `{e:#}` for source chains.
- Devlog under `devlog/stage-1-ux-layout-shell/`.

## Step 5 — Out of scope

Do NOT touch:
- B2 / B3 / B4 / B5 / B6 / B7 / B8 — separate sprints.
- C / D / E streams.
- Any heightmap / brush / procgen kernel logic — UI only.
- Tool-mode variants that don't exist yet (no stubs for Splat/Metal/Feature
  beyond enum room).

## Step 6 — Critical pitfalls (read twice)

From phase-3-plan.md B1:

1. **Immediate-mode state ownership.** Tool-specific state (brush radius,
   procgen expression, F8 selection) MUST live on `App` or `Project`, NOT
   `ui.memory()`. Memory-stored state survives tool switches but loses on
   app restart.

2. **Panel add-order.** Top → bottom → left(s) → right → CentralPanel last.
   Wrong order = central viewport eats the wrong rect.

3. **Phase 2 features must keep working.** Explicit smoke test:
   - Ctrl-Z still undoes.
   - F8 placement still works (click in StartPositions tool, drag, RMB delete).
   - F1 wizard still opens via File → New project.
   - Procgen Apply still regenerates terrain.
   - Symmetry mirror axes still apply to brush strokes.

4. **Drag threshold.** Set `ctx.set_drag_threshold(Vec2::splat(8.0))` early
   in `update()`. Without it, click-place becomes a 1-point drag-paint.

## Step 7 — Exit criteria

- 2 commits on `main`: B1 implementation + rollup.
- Devlog folder `devlog/stage-1-ux-layout-shell/` filled.
- B1 checkbox ticked in phase-3-plan.md (with link to closing log).
- ADR-030 written in `docs/DECISIONS.md`.
- SRS / ROADMAP STATUS UPDATEs for the layout change.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- App opens; left strip shows 4 tool icons; Q/B/S/G accelerators work;
  Inspector swaps contents per tool; status bar shows live camera coords
  + map dims; all Phase 2 features explicitly retested per Step 6 #3.
- Final devlog log summarising what shipped + "Sprint 3 = B2+B3+B4
  (symmetry global + canvas affordances + top-bar build)" handoff note.

Start by running `git status` and reading the files in Step 1. Then sketch
the new `App::update` structure in `notes.md` before writing code.
