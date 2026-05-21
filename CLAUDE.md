# BAR Map Editor — Agent Context

A standalone desktop GUI for authoring Beyond All Reason / Recoil maps. The
full specification lives in `SRS.md` — read that first if you are new to this
project. This file is for Claude/agent context only: how to work in this repo.

## What this project is

A single-binary Rust app (egui + eframe + wgpu) that produces a playable
`.sd7` from an empty project, on Windows and Linux. PyMapConv is bundled as a
sidecar — we do not re-implement SMF/SMT compilation.

## Stack at a glance

- **Language:** Rust (stable, MSRV 1.90)
- **UI:** egui / eframe (wgpu backend)
- **GPU:** wgpu (compute shaders for brush kernels, R16 storage textures)
- **Sidecar:** PyMapConv (CC0-1.0, ships Compressonator for BC1 — no Wine needed on Linux)
- **Packaging:** 7z **non-solid** archives (SpringFiles rejects solid `.sd7`)

See `docs/DECISIONS.md` for the why behind each.

## Repository layout

```
crates/
  barme-app/   binary — egui UI shell, main entry point
  barme-core/  lib — Project model, MapSize, heightmap, undo (eventually)
  (later)
  barme-render/   wgpu pipeline (terrain mesh, DNTS preview shader)
  barme-pipeline/ PyMapConv subprocess driver, 7z packager, mapinfo serializer
  barme-mapinfo/  Lua AST + serializer for mapinfo.lua
docs/
  ARCHITECTURE.md  module map, data flow
  ROADMAP.md       Stage 0 → Stage 3 milestones (mirrors SRS §3.2)
  PITFALLS.md      the SRS §2.1 list, restated as engineering rules
  DECISIONS.md     ADR-style record of every load-bearing call
tools/         PyMapConv + Compressonator (gitignored; vendored at build time)
assets/        fixture heightmaps, test PNGs
tests/         integration tests + golden-file fixtures
SRS.md         the canonical spec — do not edit casually
```

## House rules for agents

1. **The SRS is the source of truth.** If you discover something that contradicts
   it, do NOT silently work around — open a note in `docs/DECISIONS.md` and flag
   the contradiction to the user.
2. **The pitfall list in `docs/PITFALLS.md` is non-negotiable.** Every commit
   that touches the build pipeline must respect it. Re-read it before changing
   anything in `barme-pipeline`.
3. **Don't reinvent SMF/SMT.** Compilation goes through PyMapConv. The native
   path is a Stage-3 fallback only.
4. **Heightmap dims are `64·N + 1`, not a power of two.** This is the most
   common silent corruption.
5. **Run `cargo fmt` + `cargo clippy` before any commit.** CI will enforce.
6. **Prefer small, single-purpose crates over a monolith.** Workspace already
   set up — add new crates rather than growing existing ones.

## Git workflow

- Remote `origin` is `git@github.com:Teagueanator/BARMapEditor.git` (private,
  used as a backup target during refactor-heavy sprints).
- Commit messages are terse — one-line subject, short body only if needed.
  **Never** add `Co-Authored-By: Claude` or "Generated with Claude Code"
  trailers. Plain author commit.
- After a successful commit, push with plain `git push` (no `--force` unless
  explicitly asked). PRs / other `gh` commands remain opt-in.

## Build / run

```bash
. "$HOME/.cargo/env"
cargo check                 # fastest sanity check during dev
cargo run -p barme-app      # launches the editor window
cargo test --workspace      # unit + integration tests
```

`cargo` is user-local (rustup install), not in `$PATH` by default — always
source `~/.cargo/env` first in fresh shells.

## Current state

See `docs/ROADMAP.md`. We are in **Stage 0 (validation)**.

## Things deliberately NOT in this repo

- The PyMapConv source. We vendor binaries under `tools/` at build time; the
  source lives at `github.com/Beherith/springrts_smf_compiler` (CC0-1.0).
- Recoil engine. The launcher invokes the user's installed BAR.
- Test `.sd7` archives. Keep them out of git — they are large and binary.


## Always write dev logs. 
- Inside of the devlog folder, each issue should get it's own subfolder where your notes go
- we must log all aspects of our work, all logs should have the date / time in thier file name
