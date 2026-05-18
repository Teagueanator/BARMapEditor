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

## ADR-010 — `rfd` for native file dialogs

**Status:** Accepted (2026-05-17)
**Context:** Stage 0 needs File → Open / Save dialogs to land project TOML
round-trip. The choice is load-bearing because the dialog crate decides
the *runtime* dependency story on Linux: pure-process vs system services
vs shelling out to user-installed binaries.
**Decision:** `rfd` (Rusty File Dialog) with default features. On Linux it
prefers the xdg-portal backend (DBus to `xdg-desktop-portal`, which is
already running under any modern Wayland/X11 session); on Windows it goes
through the native Win32 dialog. Single API across OSes, no extra runtime
binaries, no async required for our synchronous-blocking UI flow.
**Alternatives:**
  - Hand-rolling against xdg-portal via `ashpd` (rejected: cross-platform
    story falls apart — would need separate code paths per OS).
  - Shelling out to `zenity` / `kdialog` (rejected: not guaranteed to be
    installed, ships a "user must `apt install zenity`" failure mode that
    contradicts the single-binary distribution goal of ADR-001).
  - Pure-egui in-app file picker (rejected: reinventing OS file UX is a
    Stage 1+ scope explosion, and we lose every system feature — recents,
    sidebar, network volumes, sandboxed-portal permission prompts).
**Consequence:** `rfd` joins workspace deps with default features.
`AsyncFileDialog` deliberately not used — Stage 0 UI is synchronous, and
blocking the egui thread for the half-second a user spends in a dialog
is fine. If we ever ship a Web build (SRS §3.5 throws this in as a
"maybe"), `rfd` already has a `wasm32` backend, but that's not
exercised yet.

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

## ADR-011 — PyMapConv vendored via `scripts/fetch-pymapconv.sh`, pinned to v0.6.3

**Status:** Accepted (2026-05-17)
**Context:** Stage 0 goal #5 — get PyMapConv on disk under `tools/pymapconv/`
so the eventual subprocess driver (ADR-012, future) has something to drive.
Three orthogonal choices were on the table:
  1. **What to vendor.** The upstream `v0.6.3` release ships three artifacts:
     a Linux tarball, a Windows zip, and the source tree. Vendoring the
     **prebuilt Linux tarball** removes the Python toolchain entirely from
     the user's machine — the release is a PyInstaller-bundled ELF binary,
     not a Python script. (This contradicts both the upstream README and the
     SRS §1.2 survey table; see drift annotation in SRS line 56.) Compressonator-
     derived DXT encoders and ImageMagick ship inside the tarball at
     `tools/{dragon-dxt1,dragon-dxt5,magick}`, so the README's "install
     Compressonator + ImageMagick yourself" instruction is stale for the
     Linux prebuilt path.
  2. **How to fetch.** A shell script (`scripts/fetch-pymapconv.sh`) over
     `build.rs`. `build.rs` network access is hostile to offline builds,
     CI without network, and reproducible packaging. The script is a one-shot
     dev/install step; the build itself stays hermetic.
  3. **How to verify.** Pinned `sha256` baked into the script. Mismatch
     hard-fails. Re-running with the artifact already extracted is a no-op
     apart from a `chmod +x` sweep.
**Decision:**
  - Vendor PyMapConv **v0.6.3 linux-amd64 prebuilt** under `tools/pymapconv/`
    (gitignored — 90 MB extracted).
  - Fetch via `scripts/fetch-pymapconv.sh`: idempotent, sha256-verified,
    linux-amd64-only for now (errors loudly on other platforms).
  - Frozen snapshot, **not** upstream-tracking. Upstream is maintenance-mode
    (last commit 2024-10-30). Bumping the pin will be a deliberate decision
    in its own ADR.
  - **Pinned SHA256:** `7040c68f7a7f401e8e7613b4f51df8a8147f66ac24b717a91888fbf15d980a73`
    (verified 2026-05-17 against the GitHub release artifact).
**Alternatives:**
  - **git submodule of source repo** — rejected: would force Python + PyQt
    install on every user, defeating the single-binary distribution goal
    of ADR-001.
  - **git-lfs the tarball** — rejected: 90 MB binary in repo history forever,
    LFS quota / hosting complications, no value over a downloader script.
  - **`build.rs` fetch** — rejected: see above (offline-hostile).
  - **Bump pin per release** — rejected: upstream barely moves; pinning
    by tag and reviewing bumps deliberately is cheaper than chasing.
**Consequence:**
  - Entry point is `tools/pymapconv/pymapconv` (ELF binary). The Rust
    subprocess driver (ADR-012) `exec`s this directly with CLI flags —
    no `python3`, no `pip install`, no PyQt or Pillow on the user's
    machine. Compressonator-derived encoders are found at
    `tools/pymapconv/tools/` relative to the binary, which PyMapConv
    locates on its own.
  - Upstream `--help` is broken in v0.6.3 (argparse `_expand_help` crashes
    on `unsupported format character ')'` in some help string). Not our
    bug; the GUI form documents the full CLI surface and is the authority
    for flag wiring. The Stage 0 → ADR-012 session log captures the flag
    table verbatim so we don't re-launch the GUI to recover it.
  - The `-u --linux` flag is the "use AMD Compressonator instead of
    nvdxt.exe" toggle and is mandatory on Linux. ADR-004 collapsed the
    nvdxt risk; this flag is the concrete switch.
  - **Windows support (deferred):** the sibling
    `pymapconv.v0.6.3.windows-amd64.zip` is published on the same release.
    When we add Windows to Stage 1, the script grows a `Windows_NT-AMD64`
    case (different unzip + different bundled tool layout). Out of scope
    for Stage 0.
  - **Python source-distribution path:** explicitly NOT supported by this
    vendoring. If a future contributor needs to patch PyMapConv source,
    they fork upstream, rebuild the PyInstaller bundle, and we bump the
    pin. Don't try to mix the two.

---

## Template for new entries

```
## ADR-NNN — One-line decision

**Status:** Proposed | Accepted | Superseded by ADR-XXX
**Context:** Why we're deciding this now; what forces are at play.
**Alternatives:** What we considered and rejected, with one-line rationale.
**Consequence:** What changes in the code/process because of this.
```
