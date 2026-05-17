# Architecture Decisions

ADR-style log. Each entry: **decision**, **context**, **alternatives**, **status**.

---

## ADR-001 — Rust + egui/eframe + wgpu

**Status:** Accepted (2026-05-17)
**Context:** SRS §4 evaluates 9 stacks; Rust + egui + wgpu wins on
single-binary distribution, GPU-compute fit for the brush pipeline, and
proven precedent (Rerun viewer).
**Alternatives:** Tauri (WebKitGTK rendering inconsistency on Linux), Godot 4
(fallback, ~93 MB binary), Qt6/C++ (LGPL packaging pain), Python+PyQt
(brush latency).
**Consequence:** Workspace pinned to Rust stable 1.95+, MSRV 1.90. Pivot to
Godot 4 + HTerrain if Stage 0 reveals wgpu compute is unworkable.

## ADR-002 — PyMapConv as sidecar, not reimplementation

**Status:** Accepted (2026-05-17)
**Context:** SRS §2 — reimplementing SMF/SMT is a 3-month detour for
negligible upside. PyMapConv is the canonical compiler.
**Alternatives:** Native Rust SMF/SMT writer via `texpresso` (kept as Stage-3
fallback per SRS pivot threshold).
**Consequence:** `barme-pipeline` shells out to a vendored PyMapConv binary.
We do not vendor PyMapConv source; only its release artifact.

## ADR-003 — PyMapConv licensing — UNBLOCKED

**Status:** Accepted (2026-05-17). Supersedes the SRS §2.1 #10 / Caveats
warning.
**Context:** The SRS flagged PyMapConv as license-unresolved (no SPDX file).
Verified 2026-05-17: the upstream repo (Beherith/springrts_smf_compiler)
now carries a **CC0-1.0** LICENSE file. CC0 is maximally permissive — no
attribution requirement, no copyleft. Redistribution inside our installer is
unrestricted.
**Consequence:** The "ask Beherith for written permission" workstream from
SRS Stage 1 is removed. We still attribute Beherith in `CREDITS.md` (TBD)
out of courtesy.

## ADR-004 — Compressonator replaces nvdxt.exe

**Status:** Accepted (2026-05-17). Refines SRS §2.1 #2.
**Context:** PyMapConv now invokes AMD Compressonator (open-source, native
Linux binary) instead of the legacy NVIDIA `nvdxt.exe`. Verified via
upstream README 2026-05-17.
**Consequence:** No Wine dependency on Linux. We bundle Compressonator
alongside PyMapConv under `tools/`. The "nvdxt unavailable on Linux native
/ ARM" risk in SRS §2.2 collapses.

## ADR-005 — Workspace layout: `barme-app` + `barme-core` to start

**Status:** Accepted (2026-05-17)
**Context:** SRS §3.4 architecture splits cleanly into 4–5 crates. Starting
with two and adding `barme-render`, `barme-pipeline`, `barme-mapinfo` as
Stage 0 requires them keeps the dep graph honest and the build fast.
**Alternatives:** Single monolithic crate (rejected: encourages
cross-coupling), all five up front (rejected: most are empty for weeks).
**Consequence:** Adding a new crate = new `Cargo.toml` + entry in
`[workspace.members]`. Keep public APIs minimal until consumers exist.

## ADR-006 — Edition 2024, MSRV 1.90

**Status:** Accepted (2026-05-17)
**Context:** Stable Rust at install time is 1.95. egui 0.32 / wgpu 26 both
support edition 2024. MSRV 1.90 gives users on slightly-older toolchains
room.
**Consequence:** No 1.95-only features in `barme-core`. CI matrix should test
both 1.90 and stable.

## ADR-007 — Test fixtures are generated, not committed

**Status:** Accepted (2026-05-17)
**Context:** Stage 0 needs a 1025² 16-bit grayscale PNG (~2 MB) as a load
target, and later stages will need diffuse splats (8192² RGBA ≈ 80 MB
uncompressed), metal/type maps, etc. Committing these binaries bloats the
repo, makes diffs noisy, and ties tests to a specific blob.
**Alternatives:**
  - Commit fixtures (rejected: scales badly, git LFS is a separate setup tax).
  - Download fixtures from a release URL (rejected: tests need network).
  - Synthesize in test bodies (rejected: I/O round-trip is part of what we
    want to verify; in-memory only doesn't exercise the PNG decoder).
**Consequence:** Fixtures live under `assets/fixtures/` (gitignored). A
`gen-fixture` example binary in `barme-core` produces them deterministically
from constants. Tests that need a real on-disk PNG `cargo run --example
gen-fixture` first, or call the generator function directly. The fixture
*spec* (dims, ramp formula) lives in code, which is the part worth
versioning.

---

## Template for new entries

```
## ADR-NNN — One-line decision

**Status:** Proposed | Accepted | Superseded by ADR-XXX
**Context:** Why we're deciding this now; what forces are at play.
**Alternatives:** What we considered and rejected, with one-line rationale.
**Consequence:** What changes in the code/process because of this.
```
