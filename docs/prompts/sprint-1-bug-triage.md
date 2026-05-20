# Sprint 1 — Stream A bug triage (A1, A2, A3, A4)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`,
working tree should be clean (run `git status` first to confirm).

This is **Sprint 1** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship items **A1, A2, A3, A4** — all of Stream A.

## Step 1 — Read the context

Read these in order before doing anything else. Don't skim:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules, build commands.
2. `/home/teague/code/BARMapEditor/SRS.md` — canonical spec. §3.2 (functional
   reqs with shipped status), §3.3 (NFRs), §2.1 (pitfall list).
3. `/home/teague/code/BARMapEditor/docs/ROADMAP.md` — Stage 1 MVP shape.
4. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable engineering rules.
5. `/home/teague/code/BARMapEditor/devlog/README.md` — devlog system overview.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md` — shipped/queued.
7. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` —
   THE WORKING PLAN. Read the full Stream A section plus "Standing
   constraints" and "Note on devlog discipline."

Also glance at ADRs 022–024 in `docs/DECISIONS.md` (undo, F8 start positions,
F1 wizard) — A1 will refactor ADR-022's data model and supersede it with
ADR-033.

## Step 2 — Devlog flow (per item)

```bash
./devlog/log.sh new <slug>            # one-time, creates the feature folder
./devlog/log.sh log <slug> "starting" # session opener log
```

Slugs are listed per item in phase-3-plan.md (e.g. `stage-1-undo-bloat-fix`).
Fill the feature folder:
- `goals.md` — from the item's Scope + Success criteria.
- `theories.md` — from the item's Hypothesis if present (A1, A3 have one).
- `notes.md` — live notes.
- `logs/<timestamp>__<title>.md` — session log entries.

When an item closes:
- Tick its checkbox at the top of its section in phase-3-plan.md.
- Append `→ devlog/<slug>/logs/<closing-log>.md` to the ticked line.
- Append a STATUS UPDATE to relevant SRS / ROADMAP entries.

## Step 3 — Scope

In order, one commit per item:

1. **A1 — Undo per-stroke copy-on-first-write [ADR-033]**
   Replace per-stamp snapshots with a stroke-level bitset + copy-on-first-write.
   Bound single-stroke memory by the unioned bbox, not the sum of per-stamp
   bboxes. Adds ADR-033 (supersedes ADR-022's snapshot rule).

2. **A2 — wgpu boot log noise + Vulkan layer doc**
   Suppress benign `wgpu_hal` warnings via the tracing filter. Document the
   `radv is not conformant` Mesa cosmetic in README or a new
   `docs/RUNTIME-WARNINGS.md`. No ADR.

3. **A3 — Procgen perf tune**
   Hoist `HashMapContext` out of the inner loop in
   `barme_core::procgen::generate`. Target: 16-SMU procgen <100 ms (currently
   ~734 ms). No ADR.

4. **A4 — Procgen live syntax check**
   Parse-only `evalexpr::build_operator_tree` on every TextEdit change;
   green/red indicator next to Apply. Depends on A3. No ADR.

Then a **5th rollup commit**: STATUS UPDATEs in SRS / ROADMAP, tick 4 boxes
in phase-3-plan.md, closing devlog log.

## Step 4 — Standing constraints (do not violate)

- `source ~/.cargo/env` in fresh shells.
- Before EVERY commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer in commits.
- Terse commit subjects (see `9891c39`, `bb83be2` for style).
- Local-only — no `git push`, no PRs, no `gh`.
- SRS is source of truth — annotate inline with STATUS UPDATE on contradiction; never silently work around.
- Tracing: `error!` / `warn!` / `info!` / `trace!`. UI error strings use `{e:#}` for `#[source]` chains. Bytes/sizes go in structured fields.
- Every item gets a devlog feature folder via `./devlog/log.sh new`.

## Step 5 — Out of scope

Do NOT drift into:
- Stream B (UX overhaul) — Sprint 2 onward.
- Stream C / D / E.
- Refactors not needed by A1–A4.
- F4–F10 features.
- Pushing or publishing anything.

If you finish A1–A4 with energy to spare, **stop**. Do not start B1. Write
a thorough closing devlog log and a "next session" note instead.

## Step 6 — Exit criteria

- 5 commits on `main`: one per item (A1–A4) + rollup.
- 4 devlog feature folders with goals/theories/notes/logs filled.
- 4 checkboxes ticked in phase-3-plan.md.
- ADR-033 in `docs/DECISIONS.md`.
- SRS § NFR-Memory annotated for the undo-cap fix.
- ROADMAP "Phase 3 known bugs" struck for A1–A4.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green on final commit.
- Final devlog log summarising what shipped + "Sprint 2 = B1 (layout shell)" handoff note.

Start by running `git status` and reading the files in Step 1. Then begin A1.
