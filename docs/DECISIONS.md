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

## ADR-008 — Coord system, world units, default height scale

**Status:** Accepted (2026-05-17)
**Context:** Stage 0 renderer needs a coordinate convention before any
vertex math goes into the codebase. Three things to lock in: axis
orientation, the world unit per heightmap pixel, and a default Y scale for
the u16 sample. All three are load-bearing — flipping any later means
re-doing camera, mesh, and (later) brush-pipeline math.
**Decision:**
  1. **Y-up, left-handed.** Matches Spring/Recoil's in-engine convention so
     authors form mental models that survive the import-into-engine step.
     +X east, +Z south. Use `glam::Mat4::look_at_lh` / `perspective_lh`.
  2. **World unit = elmo.** Same unit as `MapSize::elmo_extents()`.
  3. **8 elmos per heightmap pixel** on X/Z. Derived from Spring:
     `1 SMU = 64 hm-pixels = 512 elmos` → `512 / 64 = 8`. A pre-compact
     prompt mistakenly said "1 elmo per pixel" — that would make a 16×16
     SMU map render 8× too small. Corrected here; the canonical source is
     `MapSize::HEIGHTMAP_PER_SMU` vs `MapSize::ELMOS_PER_SMU`.
  4. **Default max height = 256 elmos** (u16 sample `65535` → `y = 256`).
     This is a Stage 0 visual default, not a Spring engine constant. It's
     exposed in the side panel as a drag so an author can dial in plausible
     mountains. Real maps set this in `mapinfo.lua` (`maxHeight`); we'll
     wire that as the source of truth when the mapinfo editor lands.
**Alternatives:**
  - Right-handed Y-up (rejected: forces a sign flip every time we compare
    coords to Spring docs / in-engine debug output).
  - 1 hm-pixel = 1 world unit, dimensionless (rejected: would mean the
    renderer can't share scalar fields like "height in elmos" with the rest
    of the project model, and would surprise anyone reading the code who
    knows the SMU math).
**Consequence:** The render module's mesh builder uses `elmos_per_pixel =
8.0` and `height_scale: f32 = 256.0` as defaults. Camera math goes through
glam's `_lh` variants. If we ever swap to right-handed for tooling
ergonomics, this ADR is what we supersede.

## ADR-009 — egui/eframe/wgpu bumped from 0.32/26 to 0.33/27

**Status:** Accepted (2026-05-17)
**Context:** First `cargo check` with the Stage 0 renderer pulled `naga
26.0.0`, which fails to compile against `codespan-reporting 0.12` because
`naga::error::ShaderError::Display::fmt` passes `&mut String` where
`term::emit` (with `termcolor` feature) requires `&mut dyn WriteColor`.
The bug is fixed upstream in `naga 27` (the `DiagnosticBuffer` inner type
is now `NoColor<Vec<u8>>` when `termcolor` is on). There is no `naga 26.0.x`
patch release.
**Alternatives:**
  - Patch naga via `[patch.crates-io]` to a gfx-rs commit (rejected:
    bespoke source pin, breaks reproducible builds).
  - Vendor naga locally with the fix (rejected: maintenance tax for a
    bug we don't own).
  - Bump to `eframe 0.34 / wgpu 29` (rejected for now: eframe 0.34
    splits `App::update` into `logic` + `ui`, which would mean rewriting
    the panel layout. The renderer goal is Stage 0; we don't need that
    churn yet).
**Consequence:** Workspace pins are now `eframe 0.33`, `egui 0.33`,
`wgpu 27`. The `App::update(ctx, frame)` API is unchanged from 0.32, and
the wgpu 27 `PipelineLayoutDescriptor`/`RenderPipelineDescriptor` field
names match what the renderer already wrote. CLAUDE.md "Stack at a glance"
should be re-read with this in mind (it lists "wgpu" without a pin; no
edit needed). Bumping again past 0.33 will require porting to the new
eframe `App` trait.

---

## Template for new entries

```
## ADR-NNN — One-line decision

**Status:** Proposed | Accepted | Superseded by ADR-XXX
**Context:** Why we're deciding this now; what forces are at play.
**Alternatives:** What we considered and rejected, with one-line rationale.
**Consequence:** What changes in the code/process because of this.
```
