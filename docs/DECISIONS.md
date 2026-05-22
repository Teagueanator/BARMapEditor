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

**Correction (2026-05-17, later same day):** the recon note above
claimed the bundled `tools/{dragon-dxt1,dragon-dxt5,magick}` collapsed
the "install Compressonator yourself" requirement on Linux. **That was
wrong.** Those bundled binaries are auxiliary GUI converters used by
PyMapConv's "convert individual texture" feature, not by the
`--linux`-mode compile path. The compile path shells out to
`CompressonatorCLI` literally (upstream `src/pymapconv.py` lines 828 +
1032; `os.system(cmd)` with no path override). Without it on `PATH`,
the compile fails at the minimap DXT step with
`sh: 1: CompressonatorCLI: not found`. Compressonator is now vendored
separately under `tools/compressonator/` per ADR-014.

## ADR-012 — `barme-pipeline` crate + PyMapConv subprocess driver

**Status:** Accepted (2026-05-17)
**Context:** Stage 0 goal #6 needs Rust code that drives the vendored
PyMapConv (ADR-011) to produce `.smf` + `.smt`. Per ADR-005 we add a new
workspace crate rather than growing `barme-core`: the pipeline owns
subprocess concerns, error surfaces meaningful to the eventual UI, and
output-path semantics that have nothing to do with the in-memory project
model.
**Decision:**
  - New crate `crates/barme-pipeline`. Depends on `barme-core` (Project,
    MapSize) and `image` (BMP encoder for synthesized test assets).
    Does *not* depend on egui / eframe / wgpu — pipeline must stay
    headless-testable.
  - Public surface:
    `PyMapConvDriver::vendored(repo_root) -> Result<_, PyMapConvError>` +
    `compile(CompileInputs) -> Result<CompileOutputs, PyMapConvError>`.
  - `PyMapConvError` (thiserror): `BinaryMissing`, `Spawn`,
    `NonZeroExit { status, stdout, stderr }`, `MissingOutput`. The
    UI surfaces the full captured stdout/stderr on failure — PyMapConv's
    own diagnostics are how authors will debug bad inputs.
  - **Working directory = the binary's parent.** PyMapConv resolves
    `./resources/geovent.bmp` and the bundled `tools/{dragon-dxt1,
    dragon-dxt5,magick}` relative to its cwd, not relative to its
    `argv[0]`. Setting cwd to `tools/pymapconv/` keeps those resolutions
    correct without rewriting any flags.
  - **Outputs absolute.** `-o` takes an absolute path under the caller's
    chosen `out_dir`, so a tempdir-scoped integration test stays
    hermetic regardless of where pymapconv decides to look.
  - **Minimum flags on Linux:** `-o`, `-t`, `-a`, `-x`, `-n`, `-u`
    (per ADR-011). Everything else is optional and not exposed in v0.
  - **Buffered stdout/stderr.** v0 `Command::output()`. Streaming-to-
    tracing for live progress is Stage 1 ergonomic polish.
  - **Post-condition check.** After exit code 0, verify the expected
    `.smf` + `.smt` exist; otherwise return `MissingOutput`. Defends
    against the failure mode where pymapconv silently writes nothing
    on subtle input errors.
  - **Integration test is `#[ignore]`-gated.** `cargo test --workspace`
    stays hermetic and offline. End-to-end compile runs via
    `cargo test -p barme-pipeline -- --ignored`.
**Alternatives:**
  - **Grow `barme-core`** — rejected: subprocess + image-encoder deps
    don't belong in the data-model crate; would force core's consumers
    (eventually a Web preview build per SRS §3.5 "maybe") to pull
    process-spawning code they don't need.
  - **Streaming output via threads + channels** — rejected for v0: adds
    machinery before there's a UI to consume it. Buffered output is
    fine for headless tests and a "compile spinner" UI.
  - **Build a Rust wrapper around the Python pymapconv source directly
    (PyO3)** — rejected: ADR-011 explicitly does *not* vendor source;
    the PyInstaller bundle is the integration boundary.
**Consequence:**
  - Two-crate workspace becomes three: `barme-app`, `barme-core`,
    `barme-pipeline`.
  - `image` workspace dep gains the `bmp` feature (encoder) so the
    integration test can synthesize a 1024² stub texture without
    committing a binary fixture (ADR-007).
  - PyMapConv exit-code semantics observed empirically and recorded in
    the session log: status 0 = success, non-zero on any error PyMapConv
    can detect at parse time. (No multi-tier exit-code language — it's
    just "did it work or not".)
  - **Dim math correction** versus the previous session's flag table:
    the minimum-legal compile is a 1024×1024 BMP + 129×129 heightmap
    (= SMF mapx=128 = BAR 2 SMU), not the "65×65" the post-compact
    prompt suggested. See session log for derivation.

## ADR-013 — `mapinfo.lua` emit + `.sd7` packaging via system `7z`

**Status:** Accepted (2026-05-17). **Superseded 2026-05-18 (C2 /
ADR-029)** for the *mapinfo emitter half*: the hand-rolled string
formatter is replaced by a Lua AST in `barme-pipeline::lua_ast` and a
schema-driven renderer in `barme-pipeline::mapinfo` (consumes the C1 /
ADR-028 typed schema). Three sidecar emitters
(`mapconfig/map_metal_layout.lua`, `mapconfig/map_startboxes.lua`,
`mapconfig/featureplacer/features.lua`) join `mapinfo.lua` in the
staging tree. **The packaging half** of this ADR (system `7z`,
`-ms=off`, post-build `Solid = -` check, staging layout, PITFALL #7
defence at source) remains in force. **STATUS UPDATE 2026-05-18 (C1 /
ADR-028):** the "minimum-viable, hand-rolled string formatter"
language is superseded by ADR-028 for the *data shape*. The
typed schema now lives at `crates/barme-core/src/mapinfo_schema.rs`
and is the canonical model F9 + C8 lint will consume. **The
emitter** (this ADR's other half) is *unchanged* in C1 — Sprint 6 /
ADR-029 (now landed) swapped it for a Lua-AST emitter + three-file
convention.
**Context:** ADR-012 produces a `.smf` + `.smt`. Recoil needs them
wrapped in a non-solid 7-Zip archive named `.sd7`, alongside a
`mapinfo.lua` that names them. PITFALL #9 (SpringFiles silently rejects
solid `.sd7`) and PITFALL #7 (pink-map trap if `smtFileName0` doesn't
match) both bear directly on this decision.
**Decision:**
  - **Hand-rolled mapinfo.lua emitter, not a Lua AST.** Lives in
    `barme-pipeline::mapinfo`. Writes a minimum-viable file based on a
    `Project`: name / shortname / version / mapfile / smf.smtFileName0
    (always `maps/<name>.smt` to keep PITFALL #7 closed) / minheight /
    maxheight, plus reasonable defaults for the keys Recoil refuses to
    boot without. A real Lua-AST emitter is the future `barme-mapinfo`
    crate's job (per the architecture in CLAUDE.md).
  - **Shell out to system `7z` for packaging.** Resolution order:
    `7zz`, `7z`, `7za`. `7zr` is skipped (it's read-only / `.rar`-style
    minimal). Missing binary → `Sd7Error::SevenZipMissing` with a
    suggested install command.
  - **`-ms=off` is mandatory** — the literal PITFALL #9 flag.
    `-t7z -mx=9` round out the create command. Run from inside a staging
    tempdir so the archive's root contains `maps/` and `mapinfo.lua`
    directly (not nested under a top-level dir).
  - **Verify non-solid post-build.** `7z l -slt <out>.sd7` and parse the
    `Solid = -` header. If it's `+` (solid), return an `Sd7Error::Solid`
    — defends against a future flag-name change or wrong-binary regression.
  - **Staging layout:** `<stage>/maps/<name>.smf`, `<stage>/maps/<name>.smt`,
    `<stage>/mapinfo.lua`. Matches what Recoil's vfs scan expects.
**Alternatives:**
  - **`sevenz-rust` / `sevenz-rust2` crate** — rejected: non-solid
    output mode has had upstream bugs reported, and SpringFiles
    silently-reject behaviour (PITFALL #9) means we'd discover any
    drift only after a real upload. Shelling out to the system binary
    is the trusted reference path for now. Revisit if cross-platform
    distribution pressure (Windows packaging) makes the system-binary
    dependency painful.
  - **Bundle our own 7-Zip binary under `tools/`** — rejected: adds an
    install vector + license tracking for a tool every Linux distro
    already packages. Reconsider in Stage 1 if rejecting users without
    `7zip` installed becomes a real friction point.
  - **Real Lua AST emitter now** — rejected: minimum-viable mapinfo is
    ~25 lines of formatted output; the AST emitter has value only when
    the editor UI needs round-tripping handwritten mapinfo files,
    which is a Stage 1+ concern.
**Consequence:**
  - `barme-pipeline` uses the `which` crate for 7z discovery.
  - End-to-end public surface:
    `barme_pipeline::build_sd7(project, hm_png, tex_bmp, out_path) ->
    Result<PathBuf, BuildError>`. The integration test exercises this
    end-to-end, producing a real `.sd7` and asserting `Solid = -`.
  - **System dependency declared.** README / install docs (Stage 1
    polish) need a line about `apt install 7zip` (or distro equivalent)
    being required *to package* — not to run the editor's GUI.
  - PITFALL #7 (pink-map on rename) defended at the source: the
    emitter always derives `smtFileName0` from the same `name` field
    the SMT is written with. There is no path that lets them diverge.

**Amendment (2026-05-17, Stage 0 goal #7):** the emitter's field set
is calibrated to the *intersection of three independent gates*, not
just engine docs:

1. **Recoil engine scanner** — only `name`, `smf.smtFileName0`, and
   `teams[*].startPos` strictly required (per the
   `burnhamrobertp/97cae4d300e675ca261e661fc58266d1` reference gist).
2. **Chobby map browser** (`beyond-all-reason/BYAR-Chobby` →
   `LuaMenu/widgets/gui_maplist_panel.lua`) — needs `modtype == 3`;
   filters unofficial maps from multiplayer lobbies (visible in
   Skirmish only).
3. **BAR mod gadgets** (`beyond-all-reason/Beyond-All-Reason` →
   `luarules/gadgets/*.lua`) — read mapinfo subtables directly without
   nil-checking; missing subtable → gadget load crash → game hangs at
   waiting-for-players. First hit: `unit_sunfacing.lua` line 44 reads
   `mapinfo.lighting.sundir` unconditionally.

The emitter therefore includes `lighting = { sundir = {…} }` even
though the engine has defaults for everything in the lighting block.
Expectation: the subtable set will grow as we discover more
gadget nil-derefs. Add a regression test in `barme-pipeline::mapinfo::tests`
for each new field, naming the gadget that forced it. See
`docs/PITFALLS.md` §"BAR Chobby + mod-gadget mapinfo expectations".

## ADR-014 — Compressonator CLI vendored via `scripts/fetch-compressonator.sh`, pinned to V4.5.52

**Status:** Accepted (2026-05-17). Refines ADR-004; corrects an
inaccurate inference in the original ADR-011 (see ADR-011's correction
note).
**Context:** PyMapConv on Linux (`-u/--linux`, per ADR-011) invokes
`CompressonatorCLI` by name via `os.system()` in upstream
`src/pymapconv.py` lines 828 (minimap) and 1032 (tiles). There is no
flag, env var, or config knob to point at a custom binary path. ADR-004
anticipated bundling Compressonator; ADR-011 missed that the PyMapConv
tarball does not include it (the bundled `tools/dragon-dxt*` + `magick`
are unrelated GUI converters). The driver's first end-to-end run in
this session surfaced `sh: 1: CompressonatorCLI: not found` and forced
the issue.
**Decision:**
  - Vendor AMD Compressonator CLI **V4.5.52 linux-amd64** under
    `tools/compressonator/` (gitignored — ~20 MB extracted; already
    covered by the pre-existing `/tools/compressonator/` rule in
    `.gitignore`).
  - Fetch via `scripts/fetch-compressonator.sh` — same shape as
    `scripts/fetch-pymapconv.sh`: `set -euo pipefail`, SHA256-pinned,
    `mktemp -d` + trap, idempotent, linux-amd64-only.
  - **Pinned SHA256:**
    `70c9cdb27a19875df03766f349864951a749a44c0f5c001c33903944465f6b97`
    (verified 2026-05-17 against the GitHub release artifact).
  - Upstream entry point is a bash launcher (`compressonatorcli`,
    lowercase) that sets `LD_LIBRARY_PATH` and execs
    `compressonatorcli-bin`. PyMapConv invokes `CompressonatorCLI`
    (camelcase). Linux is case-sensitive, so the fetch script creates a
    sibling `CompressonatorCLI` → `compressonatorcli` symlink.
  - The Rust driver
    (`barme_pipeline::PyMapConvDriver::vendored`) prepends
    `tools/compressonator/` to the child's `PATH` so PyMapConv
    resolves the call. `PyMapConvError::CompressonatorMissing` fails
    fast with an actionable "run the fetch script" message.
  - Frozen pin per ADR-011's pattern. Bumping is a deliberate ADR.
**Alternatives:**
  - **Apt `compressonator` package** — rejected: not in Ubuntu /
    Debian main repos; would push the install vector onto every
    contributor.
  - **`.deb` from the same release** — rejected: requires `sudo dpkg
    -i` and pollutes `/usr/bin/`. The tarball gives the same binary in
    a relocatable layout.
  - **Build Compressonator from source** — rejected: ~1 GB of
    CMake/OpenCV/Qt-Linguist build deps for a tool we treat as an
    opaque DXT compressor.
  - **Patch PyMapConv to accept a custom Compressonator path** —
    rejected at v0: forks the upstream we explicitly chose to track
    untouched (ADR-002 / ADR-011).
**Consequence:**
  - Stage 0 vendor footprint is now two binary trees: PyMapConv
    (~90 MB) and Compressonator (~20 MB). Both gitignored. First-time
    setup is two scripts: `./scripts/fetch-pymapconv.sh &&
    ./scripts/fetch-compressonator.sh`. Worth folding into a single
    `scripts/setup.sh` in Stage 1 polish.
  - The PyMapConv driver constructor now requires *both* binaries to
    be present. A missing-Compressonator error mentions the right
    fetch script, not the wrong one.
  - **Upstream Linux multi-thread bug discovered while wiring this
    up:** PyMapConv v0.6.3 with `numthreads > 1` (default 4) tries to
    read tile DDS files from `temp/thread{n}/temp{i}.dds` (Windows
    multi-thread layout) even when the Linux path wrote them flat into
    `temp/temp{i}.dds`. Workaround: the driver always passes `-q 1`
    (the previous session's flag table mislabelled `-q` as "Win only" —
    it's in scope on Linux too, and forcing it to 1 dodges the
    read-back mismatch). Source: v0.6.3 `src/pymapconv.py` lines
    960-986.
  - **Upstream Linux exit-code quirk:** PyMapConv exits with status 1
    even after a successful compile ("All Done!" then exit 1) — the
    bundled Qt event loop misbehaves when no display is held open. The
    driver treats artifact presence (`.smf` + `.smt`) as the success
    contract; non-zero exit is logged at `warn` and ignored when both
    artifacts wrote. If artifacts are missing AND the exit was
    non-zero, the typed `NonZeroExit` error still fires with captured
    streams.
  - **Windows path (deferred to Stage 1):** the sibling asset
    `compressonatorcli-4.5.52-win64.zip` exists on the same release
    and needs a different unzip target layout. Out of scope for
    Stage 0.

## ADR-015 — `barme-app::launcher` module + "Build & Install" UI button

**Status:** Accepted (2026-05-17)
**Context:** Stage 0 goal #7 was validated via a hardcoded
`barme-pipeline/examples/build_smoke.rs` that wrote straight into
`/home/teague/.local/state/Beyond All Reason/maps/`. Stage 1 needs that
same flow exposed through the editor UI, with a cross-platform path
resolver instead of a baked-in Linux path. Three orthogonal choices:
  1. **Module vs new crate.** ADR-005 prefers small crates, but the
     launcher is ~150 LOC of `directories`-crate lookup + `std::fs::copy`,
     with no consumer other than the UI. A new crate adds three
     `Cargo.toml`s of overhead for negligible API surface.
  2. **Path resolution source.** Re-implement from scratch, or mirror the
     canonical reference. `beyond-all-reason/spring-launcher`'s
     `src/write_path.js` is the Electron-side authority for where BAR
     stores user-writable state — matching its behaviour exactly keeps the
     editor drop-in compatible with whatever the lobby is using.
  3. **Install semantics.** Copy vs symlink. Symlinks need Developer Mode
     on Windows; BAR's archive scanner doesn't distinguish.
**Decision:**
  - **Module** at `crates/barme-app/src/launcher.rs`. Public surface:
    `bar_maps_dir() -> Option<PathBuf>`,
    `install_sd7(src, dst_dir) -> Result<PathBuf, LauncherError>`,
    `build_and_install(driver, project, hm, tex_opt, dst)`.
    `LauncherError` (thiserror): `Pipeline` (wraps
    `barme_pipeline::BuildError`), `Io { path, source }`, `TextureSynth`.
    Tracing throughout (info on lifecycle, error on unhappy paths).
  - **Linux path lookup** mirrors `spring-launcher` precedence using
    `directories::BaseDirs::state_dir()` + `UserDirs::document_dir()`:
    1. `$XDG_STATE_HOME/Beyond All Reason/maps`
    2. `$HOME/Documents/Beyond All Reason/maps` (legacy migration check)
    3. `$HOME/.local/state/Beyond All Reason/maps` (belt-and-braces fallback)
    Probes for the first existing candidate; falls through to the highest
    priority one when none exist (created on first install).
  - **Windows / macOS:** returns `None`. spring-launcher's Windows path
    is `app.getAppPath()/../../data` — portable next to the install dir
    with no fixed system anchor. The UI surfaces the `None` as
    "could not locate BAR maps dir — pick one manually (Stage 1)" and a
    user-pick file-dialog fallback is the Stage 1 polish.
  - **Copy, not symlink.** Overwrite-in-place so re-running on the same
    project is idempotent.
  - **Texture fallback** in `build_and_install`: `texture_bmp = None` →
    synthesise a flat grey `(128,128,128)` BMP at the project's texture
    dimensions. Mirrors what `examples/build_smoke.rs` does so the UI can
    ship before F4 (DNTS splat painting) lands.
  - **UI:** "Build & Install to BAR" button in the side panel and a
    sibling under the existing Build menu. Disabled until a heightmap is
    loaded. Result rendered as a coloured status line under the button —
    green with the installed path on success, red with the error on
    failure. Verbose diagnostics go to stderr via the existing
    `tracing_subscriber` setup.
  - **`examples/build_smoke.rs` retained.** It's still the cleanest
    headless smoke for `barme-pipeline` standing alone, and it
    cross-checks `launcher::build_and_install` (different driver code
    path, same pipeline + 7-Zip plumbing).
**Alternatives:**
  - **`barme-launcher` crate** — rejected (see Context #1).
  - **Hand-rolled env-var lookup** — rejected: `directories` handles the
    `$HOME` unset edge case correctly and the Linux-only `state_dir()`
    API is exactly the primitive we need. The macOS `None` return is
    honest where a hand-rolled `$HOME/Library/...` guess would be a lie.
  - **Pre-bake the maps path into project config** — rejected: every
    user would set it once, then the lobby moves and nothing works.
    Probing matches the lobby's behaviour instead.
  - **Symlinks** — rejected per Context #3.
  - **Block install on Windows entirely** — rejected: the function-level
    return-`None` is friendlier (UI can offer a pick-a-dir alternative)
    than a compile-time `#[cfg]` gate.
**Consequence:**
  - `barme-app` gains `barme-pipeline`, `directories`, `image`,
    `tempfile`, `thiserror` as direct deps (all previously in workspace
    deps, just promoted to direct).
  - `App` state grows `last_install: Option<Result<PathBuf, String>>` for
    the side-panel status line.
  - Three unit tests in `crates/barme-app/src/launcher.rs`:
    Linux-candidate shape, install creates dir + copies, install
    overwrites existing.
  - The Stage-1-polish punt list grows by one: a "pick BAR maps
    directory…" file dialog for non-Linux + the persisted preference
    that goes with it.
  - **Windows support (deferred to Stage 1):** the function still
    compiles cleanly on Windows targets via `#[cfg(not(target_os =
    "linux"))]`; it just returns no candidates. Cross-compilation/CI is
    not blocked.

## ADR-016 — Stage 0 go/no-go: PROCEED to Stage 1

**Status:** Accepted (2026-05-17)
**Context:** Stage 0 was scoped as a validation prototype for the
Rust + egui/eframe + wgpu stack with PyMapConv as sidecar. The SRS
prescribes a fallback to Godot 4 + HTerrain if any of three gate legs
fails: **Tooling** (workspace + render + project I/O), **Bridge**
(Rust ↔ PyMapConv contract), **Engine** (Recoil accepts our `.sd7`).
All eight Stage 0 goals are now ticked; this ADR records the decision
and the surprises that informed Stage 1 scope.
**Decision:** **PROCEED to Stage 1.** All three legs of the gate are
empirically green:

- **Tooling** (goals 1–5 + ADR-001/005/006/007/008/009/010):
  Workspace compiles + runs on Wayland; `cargo run -p barme-app` opens
  a window; heightmap loads from 16-bit PNG; wgpu renders the
  meshed terrain (single draw call, orbit camera); `Project` TOML
  save/load round-trips via `rfd` dialogs; PyMapConv + Compressonator
  vendored under `tools/` via SHA-pinned fetch scripts.
- **Bridge** (goal 6 + ADR-011/012/013/014):
  `barme-pipeline::build_sd7` drives PyMapConv to produce real
  `.smf` + `.smt`, emits a minimum-viable `mapinfo.lua`, and packages
  a non-solid `.sd7` via system 7-Zip. Integration test
  `tests/build_sd7.rs` exercises the whole chain; PITFALL #9 defended
  by post-build `Solid = -` parse.
- **Engine** (goal 7 + ADR-013 amendment):
  `teague-test-1.sd7` (8 SMU) loaded cleanly in BAR Skirmish; user
  placed a commander and ran a full game loop against SimpleAI.
  Latest infolog (`20260518024144_infolog.txt`) clean — no LuaRules
  errors, no nil-derefs, only benign Chobby `GetMinimapImage not
  found` warnings.

**Stage 0 surprises that should inform Stage 1 scope:**

1. **Three-gate mapinfo model.** What "minimum mapinfo" means depends
   on whether you're satisfying the engine scanner, the Chobby
   browser, or BAR mod gadgets — each has independent requirements.
   The emitter's field set is empirically calibrated; the
   `barme-mapinfo` crate (per `docs/ARCHITECTURE.md`) needs to inherit
   that calibration rather than treating the `burnhamrobertp` gist as
   the schema spec. See `docs/PITFALLS.md` §"BAR Chobby + mod-gadget
   mapinfo expectations".
2. **Compressonator is a separate vendor.** ADR-004 anticipated
   bundling but ADR-011 missed that PyMapConv's `--linux` path shells
   out to `CompressonatorCLI` by name with no override. Vendored
   separately under `tools/compressonator/` (ADR-014). Stage 1 polish
   item: a single `scripts/setup.sh` running both fetch scripts.
3. **PyMapConv v0.6.3 quirks.** `-q 1` is mandatory on Linux to dodge
   the multi-thread read-back bug; exit code 1 on success is the Qt
   event loop, not a real failure. Both are workarounded in the
   driver; bump-the-pin ADR will need to re-test both.
4. **Chobby certification.** Unofficial maps only show in Skirmish.
   Stage 1 UX needs to make this clear in the "Build & Install"
   completion state (e.g. "Installed — visible in Skirmish only;
   official maps go through `maps-metadata` PRs").

**Alternatives considered:**
  - **Pivot to Godot 4 + HTerrain.** Threshold per SRS §"Pivot
    thresholds": "PyMapConv stops being maintained, or licensing
    reverses" — neither happened (ADR-003 confirmed CC0; vendoring
    works). "Recoil changes SMF format" — no change. "Brush latency
    > 16 ms on Intel iGPU" — brush sculpting isn't built yet, but
    Stage 0 didn't surface any compute-shader showstoppers.
  - **Extend Stage 0** (e.g. add brush sculpting before declaring
    gate). Rejected: the three-leg gate as written *is* met. Adding
    brushes to "validation" stretches the prototype into the MVP and
    makes the no-go decision harder to make crisply.
**Consequence:**
  - `docs/ROADMAP.md` Stage 0 section fully ticked + a "✅ Stage 0
    complete (2026-05-17)" stamp added.
  - `devlog/stage-0-validation/goals.md` goal #8 ticked.
  - **Stage 1 entry point** is ADR-017 (next free) — likely the
    brush-sculpting compute-shader pipeline since that's both the
    centerpiece (per SRS F2) *and* the riskiest unverified path
    (wgpu compute on the dev box hasn't been exercised yet). The
    launcher + "Build & Install" loop is in place to validate sculpt
    output in-engine end-to-end from day one of Stage 1.
  - **Stage-1 polish punt list** (carried forward from this session
    + earlier Stage 0 logs):
    - `scripts/setup.sh` running both fetch scripts.
    - Windows fetch paths for both vendors.
    - Streaming PyMapConv stdout/stderr to `tracing` for a live UI
      progress strip.
    - "Pick BAR maps directory…" file-dialog fallback for non-Linux
      hosts (the launcher's Windows path).
    - Surface PyMapConv's auxiliary minimap previews (`<name>.jpg` /
      `.png`) for the project browser.
    - `mapinfo.lua` lint pass (PITFALL #6) — owned by future
      `barme-mapinfo` crate.
    - Hermetic CI: `#[ignore]`-gated tests on a separate cron job
      that fetches vendors first.
    - `shellcheck` over `scripts/*.sh`.
    - F21 + F22 + F23 from this session's SRS update: light/dark
      theme toggle, bottom CPU/memory status bar, and the
      user-asset library (F23 deferred to v2 — design ADR first).

## ADR-017 — GPU-resident heightmap as `r16uint` storage texture

**Status:** Accepted (2026-05-17). First Stage 1 ADR.
**Context:** The Stage 0 renderer baked Y into a `Vec<Vertex>` and rebuilt
both vertex+index buffers on every `height_scale` change
(`render::upload_mesh`). Live brush sculpting (next commit) writes a few
pixels per stroke — at 16 SMU that's ~1 M vertices to rebuild per stamp
on the CPU path, which is untenable for the SRS NFR-Performance
"≤ 8 ms stroke latency" target. The heightmap belongs on the GPU as a
sampled storage texture; the vertex shader reads it per-vertex; brush
edits become sub-rect `queue.write_texture` calls.
**Decision:**
  - Heightmap texture format: **`r16uint`**. Core wgpu (no
    `TEXTURE_FORMAT_16BIT_NORM` feature needed), matches `Heightmap`'s
    `Vec<u16>` on-disk type 1:1, no shader-side renormalisation cost
    (`f32(texel.r) / 65535.0` is one mul). `r16unorm` would have been
    semantically tidier but is feature-gated and provides zero
    practical benefit.
  - **Texture usage flags:** `TEXTURE_BINDING | COPY_DST`. Storage-binding
    flag deferred to ADR-021 — adding it speculatively here would
    couple Commit 1's correctness to a feature/format check that only
    matters for Commit 5.
  - **No vertex buffer at all.** The vertex shader derives `(px, pz)`
    from `@builtin(vertex_index)` and `textureDimensions(heightmap)`,
    then samples the texture for Y. The index buffer alone drives
    the indexed draw. Saves an upload + a buffer + an attribute slot.
  - **Bind group layout:** binding 0 = uniform (camera + params),
    binding 1 = `Texture { Uint, D2 }` visible to vertex stage.
  - **Dummy 1×1 texture at install time** so the bind group is valid
    before any heightmap loads. `upload_heightmap` reallocates +
    rebinds when dims change, then `queue.write_texture` for the
    full slice.
  - **`height_scale` moves into the per-frame uniform.** Changing it
    from the UI writes only the uniform — zero buffer or texture
    churn. The Stage 0 `App::rebuild_mesh` path is deleted.
  - **Grid index buffer rebuilds only when heightmap dims change**,
    not on every height edit. Same CW winding as Stage 0 so
    `front_face = Cw` still hides back faces.
  - **Renderer skips draw when no real heightmap uploaded yet** — the
    dummy 1×1 keeps the bind group valid but the central panel
    already shows the "Load a heightmap" placeholder text.
**Alternatives considered:**
  - **`r16unorm` storage texture** — rejected: requires
    `TEXTURE_FORMAT_16BIT_NORM` adapter feature with no functional
    upside over `r16uint` for our use case (we do our own normalisation
    in one mul).
  - **Keep vertex Y as a per-vertex attribute, update only the
    affected rows on brush strokes** — rejected: still needs a CPU-side
    `Vec<Vertex>` to slice into, still does a buffer write proportional
    to row width × edit height. Texture write is the same operation but
    without the parallel CPU mirror cost.
  - **Pull XZ from a vertex buffer (texture sample only for Y)** —
    rejected: at 1025² that's 8 MB of XZ data we'd compute and upload
    once and then keep around forever, when the shader can derive it
    from the texture dimensions for free.
  - **Use `texture_2d<f32>` via `rgba8unorm` or similar** — rejected:
    loses 8 bits of precision relative to the source data, and
    quadruples upload bandwidth.
**Consequence:**
  - `crates/barme-app/src/render.rs` heavily restructured:
    `RenderResources` now owns a `HeightmapTex { tex, view, dims }` +
    `Grid { index_buf, index_count, dims }`. `upload_mesh` →
    `upload_heightmap` (signature drops `height_scale`).
  - `crates/barme-app/src/terrain.wgsl` rewritten: vertex shader samples
    the texture; uniform shrinks to `view_proj + params(max_height,
    elmos_per_pixel)`.
  - `App::rebuild_mesh` deleted from `main.rs`; height-scale drag no
    longer triggers re-upload.
  - **Foundation for ADR-018 (brushes):** dirty-rect sub-uploads use the
    same texture via `queue.write_texture` with a `Origin3d` offset.
  - **Foundation for ADR-021 (GPU compute brushes):** the texture is the
    write target; that commit adds `STORAGE_BINDING` to the usage flags
    (which may need a fall-back to `r32uint` if `r16uint` storage isn't
    universally available — recorded as a known unknown there, not
    here).

## ADR-018 — Extensible `Brush` trait + raise/lower/smooth + dirty-rect sub-upload

**Status:** Accepted (2026-05-17).
**Context:** SRS F2 (heightmap sculpting) is Stage 1's centerpiece. The
user explicitly asked that brushes ship as a *plugin-shaped* surface ("like
Blender there are multiple different brush types so we need to have space
for them in the future"). Three brushes (raise / lower / smooth) cover the
v0 sculpting loop; the architecture has to accept flatten / erode / noise /
terrace / ramp later without touching the dispatch site or the UI. Two
orthogonal choices:
  1. **Trait + registry vs enum.** Enum is simpler but every new brush
     forces an `impl Brush for Foo` match arm edit at the dispatch site
     and an enum variant. Trait `Brush: Send + Sync + 'static` with a
     `Vec<Box<dyn Brush>>` registry inverts that: new brush = new struct
     + one `Box::new(...)` in `default_set()`, dispatch is dynamic.
  2. **CPU kernel vs GPU compute first.** CPU lets us validate UX
     (radius/strength/falloff/symmetry interaction) without a wgpu
     compute-shader rabbit hole. Port to GPU in ADR-021 *only if* the CPU
     path fails the NFR-Performance "≤ 8 ms stroke latency" budget at 16
     SMU. The dirty-rect bookkeeping introduced here is the foundation
     ADR-021 will dispatch onto.
**Decision:**
  - **`barme-core::brushes`** module with `Brush` trait, `BrushStamp` /
    `DirtyRect` value types, and `BrushRegistry` (vec of boxed brushes).
    Trait is object-safe + `Send + Sync + 'static` so a wasm-plugin
    runtime could feed in brushes from outside the crate later.
  - **Three starter brushes:** `Raise`, `Lower`, `Smooth`. Stateless unit
    structs (radius/strength flow through `BrushStamp`, not the struct).
  - **Kernel math lifted from Jandodev/bar-editor's `terrain-edit.ts`**
    (MIT, attribution comment at module head). Raise/lower:
    `delta = ±strength · STAMP_MAX_DELTA · smoothstep(1 - d/r)` where
    `STAMP_MAX_DELTA = 0.05 · u16::MAX` (≈ 20 full-strength stamps to
    saturate). Smooth: 3×3 mean blend with `mix = strength · falloff`.
    Smooth takes a snapshot of the bounding rect + 1 px margin to avoid
    propagation bias on a single pass.
  - **`DirtyRect` always returned by `apply`** (or `None` for off-map /
    zero strength). Caller uses it to scope the GPU sub-upload — one
    `queue.write_texture` per stroke instead of a full re-upload.
  - **CPU heightmap is authoritative.** `App::HeightmapState` now owns
    the `Heightmap` (was a path-only lookup that round-tripped through
    `load_png` for every redraw). Brushes mutate in place; GPU texture
    is the derived view. `build_and_install` writes the current
    in-memory state to a tempdir PNG so unsaved sculpt edits ship.
  - **Stroke handling: one stamp per frame while LMB is held in the
    central rect.** Spacing is implicit (frame rate). With a brush
    active, right-mouse-button orbits the camera; no brush selected
    keeps Stage 0 left-drag-orbits behaviour.
  - **Picking: screen-ray vs y=0 plane.** Trades altitude accuracy for
    predictability and zero per-frame compute work. Ray-vs-heightmap is
    a Stage 1 polish item. (The plane intersection is in
    `render::screen_to_world_y0`, callable from anywhere.)
  - **Sub-upload helper `render::write_heightmap_rect`** issues a single
    `queue.write_texture` with `Origin3d` offset + `bytes_per_row =
    full_w · 2` so the caller passes the full heightmap slice + the
    rect; no scratch copy needed. wgpu has no row-alignment requirement
    on `queue.write_texture`, so any rect width is fine.
**Alternatives considered:**
  - **Enum of brush kinds.** Rejected per Context #1 — adds friction
    proportional to brush count, no upside.
  - **Per-frame full texture upload.** Rejected: at 16 SMU = 1025² · 2 B
    = 2 MB, plausible at 60 FPS but pointless when the affected rect is
    typically <1 % of the heightmap.
  - **Skip the CPU mirror and let the GPU texture be authoritative.**
    Rejected for v0: save / install paths need a CPU heightmap to write
    a PNG; deferring sync to save-time means a `copy_texture_to_buffer`
    + readback dance that's better introduced alongside ADR-021's
    compute dispatches, not now.
  - **Per-brush parameter structs in `BrushRegistry`.** Rejected:
    radius/strength are universally meaningful, and brush-specific
    params (e.g. flatten's target height) can live in the kernel's own
    state in future commits without changing the trait. v0 stays lean.
  - **Async stroke processing.** Rejected: a single frame's stamp is
    microseconds at 16 SMU; threading overhead would dominate.
**Consequence:**
  - `barme-core` gains a `brushes/` sub-module. Public re-exports:
    `Brush`, `BrushRegistry`, `BrushStamp`, `DirtyRect`.
  - `Heightmap::data_mut()` added (was read-only). All callers go
    through the trait, so the new mut access doesn't leak into general
    crate consumers.
  - `crates/barme-app/src/render.rs` gains `screen_to_world_y0` and
    `write_heightmap_rect`; `OrbitCamera::view_proj_matrix` is now
    `pub` (was `fn view_proj`).
  - **UI:** "Sculpt" panel section with `Off + 3 brushes` dropdown,
    radius (8–4096 elmos) and strength (0–1) controls. Brush dropdown
    is populated from `BrushRegistry::iter()`, so adding a brush =
    a Box-new + an automatic UI entry.
  - **NFR-Performance:** unmeasured this commit. Bench numbers go into
    ADR-021's deciding section. If CPU latency clears the budget at
    16 SMU, ADR-021 becomes a deferred ticket; if not, we port the
    kernels.
  - **Symmetry (ADR-019, next commit)** plugs into `apply_brush_at` by
    replicating `BrushStamp` centers; each rect-result unions into one
    upload via `DirtyRect::union`.

## ADR-019 — Symmetry enforcement (mirror axes + N-fold rotational)

**Status:** Accepted (2026-05-17).
**Context:** SRS F3 (symmetry enforcement) is core to BAR mapmaking —
most competitive maps are mirror-symmetric or rotationally symmetric so
that no spawn position has a structural advantage. The natural shape:
one brush stamp produces N derived stamps that the kernel applies in
turn, with their dirty rects unioned for a single GPU upload. Two
orthogonal choices:
  1. **Geometric coverage.** Mirror across each map centerline,
     both diagonals, both centerlines together (quad), and N-fold
     rotational. The set is small; enum encoding is right.
  2. **Rotational fold values.** User mid-session: "if it's a three
     player map it's symmetrical in 3 quadrants from the center; if
     it's 4 then quad". So fold is a free-form integer keyed to
     player count, not a fixed list. Range 2..=12 covers FFA up to
     the point where adjacent stamps overlap into radial blur.
**Decision:**
  - **`barme_core::symmetry::SymmetryAxis` enum** with variants
    `None / Horizontal / Vertical / Quad / DiagonalMain / DiagonalAnti
    / Rotational { fold: u8 }`. Serde-serializable for project save/load.
  - **`replicate(center, extents) -> Vec<(f32, f32)>`** returns all
    derived centers including the original. World-space coords
    (elmos). Off-map results filtered out (mirror past a map edge);
    duplicates within 0.5 elmos (sub-pixel) deduplicated — handles
    the rotational-at-map-center degenerate case.
  - **Rotational math:** rotate around map center by `2π · k / fold`
    for `k = 1..fold`. `fold = 1` → identity. `fold > 1` covers all
    practical N-player layouts.
  - **UI:** symmetry dropdown in the Sculpt panel. When `Rotational`
    is selected, a `DragValue<u8>` (range 2..=12) appears for fold
    selection with a "3 = three-player, 4 = quad-player, etc." tip.
    The fold value is stashed in `App.rotational_fold` so it persists
    across toggles between rotational and non-rotational modes.
  - **Stroke integration:** `apply_brush_at` calls
    `symmetry.replicate(...)`, runs the brush at each center, folds
    dirty rects into one via `DirtyRect::union`, then one
    `write_heightmap_rect` upload covers them all.
  - **Diagonals are geometric-only.** They produce sensible output on
    square maps; on rectangular ones the reflected point may land
    off-map, in which case the `replicate` filter drops it. No
    aspect-ratio warning surfaced to the user — the UI behaviour
    (stamps just don't appear in expected places) is self-documenting.
**Alternatives considered:**
  - **Fixed `{2, 3, 4, 6, 8}` fold dropdown.** Was the initial
    implementation; user pushed back mid-commit asking for editable
    fold. Replaced with `DragValue<u8>` covering 2..=12. The original
    dropdown's "lock to N" justification (BAR maps are mostly 2/4-fold)
    doesn't outweigh the cost of being wrong for 3/5/6-player layouts.
  - **Pre-bake symmetric strokes into the heightmap on commit.**
    Rejected: hides the symmetry state from undo; doesn't survive
    a brush change mid-stroke; can't be turned off later.
  - **Per-brush symmetry override.** Rejected as YAGNI — there's no
    realistic workflow where a smooth brush wants different symmetry
    from a raise brush within the same project.
  - **Reflection across an arbitrary axis (line picker).** Rejected
    for v0: the four diagonals + horizontal/vertical cover 99% of
    real maps, and an arbitrary-axis picker is a UX rabbit hole
    (drag endpoints? type coords?) better deferred until a real
    user asks for it.
**Consequence:**
  - `barme-core` gains `symmetry.rs` with 8 unit tests covering all
    variants + the rotational-at-center degeneracy + the off-map
    filter. `SymmetryAxis` re-exported from the crate root.
  - `App` state grows `symmetry: SymmetryAxis` + `rotational_fold: u8`.
    `apply_brush_at` is now N-fold per stamp; `DirtyRect::union`
    earns its keep.
  - **F3 status:** SRS gets a STATUS UPDATE noting v1 is shipped with
    the axes + N-fold list above; arbitrary-axis lines are Stage 2.
  - **Future:** when the project file gets a symmetry field, it
    serializes the user's chosen `SymmetryAxis` directly (the
    Serde derive is already in place). New-project wizard (F1) can
    surface symmetry as a first-class choice.

## ADR-020 — Math-function terrain generator (`procgen`)

**Status:** Accepted (2026-05-17). Partial implementation of SRS F14.
**Context:** User wants `f(x, z) → height` terrain generation now
("if we want to make a hill that follows a parabola, we should be
able to enter a math function that describes the terrain"). This is
F14 territory (procedural terrain), which the SRS originally put in
Stage 2 alongside FBM + hydraulic erosion + river-carve. But the
math-function subset is small (~30 LOC of glue + one expression-eval
crate), and it unlocks the user's actual workflow: "start with a
parabolic bowl, then sculpt detail with brushes." Shipping it now
makes the brush opener immediately useful on blank projects.
**Decision:**
  - **`barme_core::procgen` module** with one public function
    `generate(expr, domain, size, min_h, max_h) -> Result<Heightmap, ProcGenError>`.
  - **Expression evaluator: `evalexpr` v13.**
    `build_operator_tree::<DefaultNumericTypes>(expr)` parses once
    (returns typed parse errors via `ProcGenError::Parse`);
    `node.eval_with_context(&ctx)` evaluates per pixel against a
    `HashMapContext` that rebinds `x` / `z` floats each iteration.
    Built-ins cover `+ - * / ^`, trig (`math::sin` / `cos` / `tan`),
    `exp` / `log`, `sqrt`, `abs`, `min`, `max`, and comparisons.
  - **Two normalisation domains:**
    `Domain::Unit` → `x, z ∈ [0, 1]` (NW=(0,0), SE=(1,1));
    `Domain::Centered` → `x, z ∈ [-1, 1]` (origin=center, ±1=edge).
    Centered is the right default for radial shapes; Unit is the
    right default for ramps.
  - **Output scaling:** `clamp(value, 0, 1) · u16::MAX`. The
    expression's range `[0, 1]` is "fraction of the height budget";
    out-of-range values clamp without erroring. NaN / Inf samples
    count as 0 with a one-shot `warn!` per generation so users
    notice degenerate input but the generation completes.
  - **Error surface:** `ProcGenError` is `thiserror`-typed —
    `Parse(EvalexprError)` at parse time, `EvalFailed { pixel,
    source }` at per-pixel-eval time, `NonNumeric { got }` if the
    expression returns a boolean or string, `Heightmap(source)`
    if `Heightmap::new` rejects the dims (shouldn't happen — we
    derive them from `MapSize`).
  - **Preset list as code, not data file.** Seven starter presets
    (flat, parabolic bowl / dome, conical peak, ridge, diagonal
    ramp, sine ripples) live in `PRESETS: &[ProcGenPreset]`.
    Selecting one fills the UI text field with the expression
    + sets the appropriate domain. Adding a preset = one entry
    in the array; the UI iterates.
  - **UI:** "Generate from formula" section in the side panel.
    Text-edit for the expression, two-radio domain picker, a
    preset combo box, an "Apply" button. Apply replaces the
    current heightmap with the generated one (re-uses
    `render::upload_heightmap` from ADR-017) and re-frames the
    camera. Errors render inline as a red label; the existing
    heightmap is untouched on failure.
  - **`build_and_install` snapshots in-memory first.** Already
    handled in ADR-018 — the in-memory `Heightmap` is the
    authoritative source for the build pipeline, not whatever
    PNG the project was loaded from.
**Alternatives considered:**
  - **`meval` crate.** Smaller, has a closure-builder ergonomics
    win, but lacks `min`/`max` and a couple of math functions we'd
    want for presets. Tip-the-scale for `evalexpr`.
  - **Hand-rolled shunting-yard parser.** ~200 LOC, zero deps.
    Rejected: `evalexpr` is well-maintained, has good error
    messages, and shipping our own bug-for-bug rebuild of an
    expression evaluator doesn't earn its keep.
  - **Defer until Stage 2** as originally planned. Rejected: user
    explicitly asked, and this is the right time to ship — the
    feature is one-day work and unlocks the brush opener.
  - **Symmetry applied to math-gen results.** Rejected per the
    plan-mode question: math expressions are symmetric if the
    expression is symmetric; forcing a fold-and-average step
    would be surprising. Symmetry stays scoped to brush strokes
    (ADR-019).
  - **Bind additional variables** (map-relative distance to
    center, polar angle). Rejected for v0: easily derivable from
    `x, z`, and starting minimal makes the surface cleaner. Add
    them when a real preset wants them.
**Consequence:**
  - `evalexpr = "13"` added to workspace deps. `barme-core`
    depends on it directly.
  - `barme-core::procgen` module + 5 unit tests (corner values,
    parse-error propagation, paraboloid shape, preset
    parse-runs-clean). Re-exported from the crate root as
    `Domain`, `PRESETS`, `ProcGenError`, `ProcGenPreset`, and
    `procgen_generate`.
  - `App` state grows `procgen_expr / procgen_domain /
    procgen_last_error`. `FileAction::ApplyProcGen` variant +
    handler `apply_procgen()`.
  - **SRS F14 STATUS:** math-function subset shipped in Stage 1.
    Remaining (FBM, hydraulic erosion, river carve) still Stage
    2. Add a STATUS UPDATE noting the partial.
  - **Future:** when the project file gains a procgen-history
    field, the expression + domain + apply-order ride alongside
    brush strokes for true reproducibility.

## ADR-021 — GPU compute brushes: DEFERRED (CPU is ~10× under budget)

**Status:** Accepted (2026-05-17). Decision is *defer*, not implement.
Re-evaluate when 32 SMU support lands (Stage 2 territory).
**Context:** ADR-018 shipped the CPU brush kernels with a scope guardrail:
"if Commit 2's CPU kernel measures ≤ 8 ms per stamp at 16 SMU (SRS
NFR-Performance), land it as a marker for future work and skip the
porting." Time to measure.

`crates/barme-core/examples/bench_brushes.rs` was added to capture this
empirically; SRS NFR-Performance is the bar.

**Measured at 16 SMU (1025×1025 = ~1 M px, release profile, ryzen 5800X3D):**

| radius (elmos) | raise   | lower   | smooth  |
|----------------|---------|---------|---------|
|            128 | 0.003ms | 0.004ms | 0.014ms |
|            256 | 0.020ms | 0.016ms | 0.051ms |
|            512 | 0.057ms | 0.065ms | 0.221ms |
|           1024 | 0.248ms | 0.246ms | 0.787ms |

The worst case is `smooth` at radius 1024 elmos (128 px radius = ~50k pixel
area, plus the 3×3 neighbour kernel) — 0.79 ms per stamp. SRS budget is
8 ms; we have **~10× headroom** even on the largest realistic brush. The
GPU port would buy nothing in user-perceptible latency at 16 SMU and would
cost:

1. An adapter-feature check for `R16Uint` storage texture access (it's
   under `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` in core wgpu, not
   guaranteed on all GPUs). Fallback would be to bump the storage to
   `R32Uint` (8 MB at 16 SMU, harmless but wasteful).
2. A compute pipeline + bind group per brush kernel, all using the same
   storage texture binding.
3. CPU mirror sync on save/install via `copy_texture_to_buffer` + async
   `buffer.map_async` — a non-trivial state machine.
4. The CPU `Heightmap` becomes the *cached* representation rather than
   the source of truth, which complicates the brush trait surface (would
   need a `&mut HeightmapView` abstraction that hides CPU vs GPU).

**Decision:** **Do not port now.** CPU kernels are the implementation
through Stage 1. The bench example is committed so anyone can re-run
the measurement, and the trigger condition for revisiting this ADR
is written down below.

**Re-evaluation triggers:**

- 32 SMU map support (Stage 2 per SRS §3.2). Heightmap grows 4×; the
  worst-case smooth stamp scales linearly with pixel area, so expected
  worst case ≈ 3.2 ms — still under budget but tighter. If we add a
  larger brush range (radius 2048+) or higher-order kernels (erosion,
  hydraulic) at that size, GPU porting earns its keep.
- A user-reported lag on a target hardware tier we haven't profiled
  (low-power Intel iGPUs, ARM laptops).
- A new brush whose CPU implementation is irreducibly slow (e.g.
  iterative erosion simulating O(N) particle passes).

**Alternatives considered:**

- **Ship the WGSL skeleton + an unused compute pipeline now.** Rejected:
  half-finished implementation, would have to be maintained against
  wgpu API changes for no current benefit. Better to write it fresh when
  the trigger condition fires.
- **Use SIMD / `rayon` parallelism on the CPU.** Possible easy 4–8× win
  if a future brush actually needs it. Cheaper than GPU porting; would
  precede ADR-021 reactivation.

**Consequence:**

- **No code change to the brush pipeline.** Brushes stay CPU-only.
- `Heightmap::data_mut()` (ADR-018) remains the brush write target;
  `render::write_heightmap_rect` does the GPU sync per stroke.
- `crates/barme-core/examples/bench_brushes.rs` is committed as the
  evidence artifact. Run with
  `cargo run --example bench-brushes --release -p barme-core`.
  The numbers in the table above are the 2026-05-17 baseline; later
  sessions should re-run before declaring "still under budget".
- **Storage-binding flag NOT added to the heightmap texture yet.**
  When ADR-021 is reactivated, that's the first patch — see ADR-017's
  Consequences for the deferral note.
- **F2 (SRS) reaches functional completeness for Stage 1** with
  raise/lower/smooth CPU brushes. Further brushes plug into the same
  trait without re-deciding any of this.

---

## ADR-022 — Undo / redo over heightmap dirty-rect snapshots

**Status:** Accepted (2026-05-17). **STATUS UPDATE 2026-05-18:** the
per-stamp `StampSnapshot { rect, before }` capture rule is **superseded
by ADR-033** (copy-on-first-write within a stroke). The high-level
contract — stamps coalesce into strokes, strokes are the undo unit,
barriers clear history wholesale, 100 MB ring cap — is unchanged. Read
ADR-033 for the new data path.
**Context:** Sculpting is exploratory — the user paints, evaluates, often
regrets. The smoothstep falloff in ADR-018 is not strictly invertible:
re-stamping with negative strength does not reproduce the original
heights because falloff is multiplicative and the kernel clamps to
`[0, u16::MAX]`. Misclicks therefore become irrecoverable, and "try a
different brush mode and see how it feels" is impossible without a
manual save-before-experiment workflow. Every comparable tool
(Photoshop, Blender, Krita) has undo because exploratory edit–evaluate
is the core loop, not an edge case.

Two design decisions follow from the ADR-018 architecture:

1. **The dirty rect is the natural diff unit.** The brush trait already
   returns a `DirtyRect` because the GPU re-upload path needs one. The
   same rect plus a copy of its pre-edit pixels is the smallest object
   that fully describes "what changed." A typical 256-elmo brush stamp
   at radius 32 px is ~4 KB; even at radius 1024 px the snapshot is
   ~512 KB — small enough that 100 strokes of headroom fits in 100 MB
   on a 16-SMU map.
2. **Stamps coalesce into strokes.** A user-perceived edit is one
   LMB-down → LMB-up, not the 60 individual stamps emitted along the
   drag. Without coalescing, Ctrl-Z would peel back one stamp at a time
   — useless for an exploratory drag and surprising as UX.

**Alternatives considered:**
- **Full-map snapshots per edit.** ~2 MB per stroke at 16 SMU
  (`1025·1025·2 B`). Simpler model, but at 32 SMU the snapshot is ~16
  MB and procgen would have to snapshot the whole new map every apply.
  Rejected on memory + uniformity grounds.
- **Command pattern (replay strokes forward).** Would require every
  brush kernel to be deterministic *and* re-runnable from the
  pre-state. The smooth kernel reads neighbours that the raise kernel
  may have moved; replay would have to remember the order of *every*
  kernel application, including symmetric stamps. The diff/snapshot
  model is the same data with simpler invariants.
- **Tile-COW heightmap (Stage 2).** When that lands, the snapshot can
  shrink further by reference-counting unchanged tiles. ADR-018's
  dirty-rect bookkeeping is the precursor; tile-COW is orthogonal.

**Barrier events.** Procgen apply, heightmap PNG load, and "new
project" *replace* the heightmap wholesale rather than mutate a sub-
rect. Capturing a 2 MB full-map diff for each is feasible but the UX
of "Ctrl-Z across a procgen swap silently restores a half-formed bowl"
is confusing; the editor barriers history at those events instead.
This is the same convention Blender uses for File → New.

**Memory cap.** 100 MB is enforced as a ring buffer (`VecDeque`):
on push, evict from the front until under cap. The cap is exposed via
`History::new(cap_bytes)` for testing; the user-facing instance always
uses the default. The eviction emits a `warn!` once per evicted entry
so the bug-where-a-stroke-balloons-into-a-gigabyte is visible.

**Linear redo.** A new edit clears the redo stack. Branching history
is occasionally requested by power users but the implementation cost
(tree visualization, branch labels) is disproportionate; no upstream
mapping tool offers it.

**Consequence:**
- New module `barme-core::undo` with `StampSnapshot`, `UndoEntry`,
  `History`. Public API is `push / apply_undo / apply_redo / barrier`.
- `Heightmap` grows `copy_rect` and `swap_rect` — slice-level row
  copies, no allocation in the hot path beyond the initial pre-edit
  capture.
- `brushes::pixel_bbox` is now `pub` so callers can pre-compute the
  unioned snapshot rect *without* applying the kernel first.
- `App.history: History`, `App.stroke: Option<UndoEntry>`. Stamps
  accumulate into the open stroke; pointer-release commits it.
- Edit menu added (first new top-level menu since Stage 0); Ctrl-Z /
  Ctrl-Shift-Z / Ctrl-Y bindings. Disabled states reflect stack
  emptiness.
- 6 unit tests cover round-trip, overlapping-stamp ordering, redo
  invalidation on new edits, barrier semantics, and cap eviction.

## ADR-023 — Project `start_positions` + F8 placement editor

**Status:** Accepted (2026-05-17). **Superseded 2026-05-18 (B6 /
ADR-032)** for the data shape: `Project.start_positions:
Vec<StartPosition>` is replaced by `Project.ally_groups:
Vec<AllyGroup>` (two-level tree). The single-team-per-symmetry-image
behaviour described here was never correct for 8v8 / 3-way FFA / 4-way
FFA maps — ADR-032 introduces presets, drag-paint, and ally-team
grouping that fix the model. The original `assign_team_ids` parity
helper survives because the same logic is useful **within** one ally
group; the legacy `team_id` field on `StartPosition` is dropped (team
ids are now positional in the flat `teams[]` pool at emission time).
The pre-Phase-3 wire format with `[[start_positions]]` and `team_id`
loads forward via the custom `Deserialize` (see
`project::ProjectWire`). The marker-rendering / hit-testing /
LMB-place-RMB-delete UX surface remains in force; B6 replaces the
single-list Inspector with a tree.
**Context:** Stage 0 closed with `barme-pipeline::mapinfo` emitting two
hardcoded teams at 25 % / 75 % along the map diagonal. That's enough to
boot a 1v1 in BAR but it's not a *map editor* feature — real BAR maps
ship with up to 12+ teams in symmetric clusters
(`gecko_isle_remake_v1.2.1.sd7` is a working reference: 16×18 SMU, 12
teams arranged across two mirror halves). F8 is the first surface where
the editor stops being a heightmap tool and starts being a map tool.

The shape of the feature is constrained by three forces already in the
codebase:

1. **Symmetry is sticky.** Users already configure mirror / quad /
   rotational symmetry for brushes (ADR-019). Placing a team under
   active symmetry must replicate it through the same `replicate(...)`
   primitive — anything else is a surprising inconsistency.
2. **The mapinfo emitter is a string formatter, not an AST.** ADR-013
   pinned that on purpose for v0. Adding `start_positions` is one more
   sorted iteration, not a new dependency.
3. **Project files must load forward.** Projects authored before this
   commit need to keep opening. `#[serde(default,
   skip_serializing_if = "Vec::is_empty")]` solves both directions in
   one annotation.

**Team-id assignment under symmetry.** When the user places one team
and symmetry replicates N − 1 mirrors, the editor needs to hand each
output position a fresh team id. BAR's `teams[]` convention is even-id
on side A, odd-id on side B. The natural mapping is: original = lowest
unused *even* id; mirror 1 = lowest unused *odd* id; mirror 2 = next
unused even; etc. This is implemented as
`start_pos::assign_team_ids(used, n)` — a pure function with unit tests
covering both empty and partially-filled `used` sets, plus the 3-fold
rotational case (3 outputs, alternating parity).

**Alternatives considered:**
- **Sequential ids without parity awareness.** Simpler but breaks the
  side convention; a quad-symmetric placement would yield
  `{0, 1, 2, 3}` on the same side. Rejected.
- **Group-id metadata on each StartPosition.** Would let "move team 0"
  also move its mirror counterparts as a unit. Deliberately deferred:
  the simple model (drag moves one position) ships now and matches what
  the gecko maintainers do (hand-place each side). Worth revisiting
  when undo grows symmetry-aware grouping.
- **Snap-to-heightmap-Y on placement.** `startPos` in `mapinfo.lua` is
  `{ x, z }` only — engine resolves Y at load time from the heightmap.
  2D placement is the canonical model; ray-vs-heightmap would only help
  the visual marker, not the data.

**Interactions and hit testing.** The central preview rect already
projects world-space points to screen via `OrbitCamera::view_proj_matrix`;
the inverse `screen_to_world_y0` from ADR-018 handles ray-vs-plane for
brush picking. We add `render::world_to_screen` as the forward direction
so the placement-mode hit test runs in screen space without a per-marker
Z-sort. Hit-test radius is 12 px, larger than the 8 px filled disc so
the click target is forgiving.

**Marker overlay.** Drawn as filled circles on top of the terrain via
`ui.painter_at(rect)`. Always rendered when any positions exist, even in
Sculpt mode, so users see them while brushing. Team-id label above each
disc. The 8-colour palette alternates warm / cool by parity so the side
convention is visually reinforced.

**Consequence:**
- `Project` grows two opt-in fields: `start_positions: Vec<StartPosition>`
  with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.
  `StartPosition` is `{ team_id, x_elmo, z_elmo }`. The model is now
  ready for `metal_spots`, `geo_vents`, `features` to land under the
  same pattern.
- `start_pos` module in `barme-core` for the id-assignment logic —
  7 unit tests cover parity, partially-used sets, and 3-fold rotation.
- `mapinfo::render` switches to emitting authored teams when present;
  falls back to the 25/75 default pair when the vector is empty. Output
  is sorted by `team_id` so diffs stay deterministic. 2 new emitter
  tests pin the contract.
- `App` gains `tool_mode: ToolMode { Sculpt, StartPositions }`,
  `start_positions: Vec<StartPosition>`, `dragging_start_pos: Option<u8>`.
  Side-panel radio selects mode; in Start-positions mode the central
  rect's LMB places / drags markers, RMB-click deletes, RMB-drag orbits.
- `render::world_to_screen` added with 3 unit tests (inverse
  consistency, off-screen rejection, camera-target sanity).
- 8-colour `team_color` palette in the app (warm = even, cool = odd).

## ADR-024 — F1 new-project wizard + rectangular MapSize refactor

**Status:** Accepted (2026-05-17).
**Context:** Stage 0 ships with a hardcoded "untitled 16×16 SMU"
in-memory project that auto-loads on every launch. That's adequate for
internal testing; it's the wrong entry point for users. Real BAR maps
are rarely square — `gecko_isle_remake_v1.2.1` is 16×18, `Quicksilver`
is 12×8 — so the wizard has to default to rectangular-capable size from
the start, not "square with optional resize later." A wizard is also
the natural surface for seeding terrain via a biome preset (the user
gets a non-empty heightmap on first click of the brush, not a wall of
"now load a fixture" hints).

The refactor of `App.map_size_smu: u32 → App.map_size: MapSize` is a
prerequisite, not the goal. Most call sites already funnelled through
`MapSize::square(self.map_size_smu)`; swapping in `self.map_size` is a
one-token edit per site. The side panel grows from a single DragValue
to two (`smu_x` × `smu_z`) and the validation messages in the Heightmap
panel use both axes. `Project.size: MapSize` already supports
rectangular — the wire format didn't change at all, this is purely an
internal representation tidy.

**Biome presets vs procgen presets.** ADR-020 already has a `PRESETS`
table for the free-form procgen UI ("pick one to fill the expression").
The F1 wizard reuses that infrastructure with a parallel `BIOMES`
table: same `expression` + `domain` fields, plus a `max_height_hint`
so a "flat plain" biome doesn't ship with a 4096-elmo height scale.
The wizard's max-height field defaults to the hint until the user
edits it directly, after which the field stops snapping to biome
defaults (`height_from_biome: bool` flag). Four biomes ship: flat
plain, parabolic bowl, cone peak, diagonal ramp. Adding new biomes is
one struct-literal entry.

**Name sanitization.** PITFALL #7 (pink-map on rename) attaches to the
project name: every downstream consumer — the `.sd7` archive name,
`mapinfo.lua` `name` / `mapfile` / `smtFileName0` fields, the SMT
filename inside the archive — must derive from the same string. The
wizard accepts free-form input but the persisted name routes through
`barme-core::project::sanitize_name(s) -> String`. Allowed characters:
ASCII alphanumeric, `_`, `-`. Anything else collapses to a single `_`
(runs are merged), and empty results map to `"untitled"`. The wizard
previews the sanitized form live below the text field so users see
what they're actually creating.

**Modal vs panel.** The wizard is an `egui::Window` anchored to screen
center with `collapsible = false`, `resizable = false`, and an X-close
that maps to Cancel. We don't use `egui::Modal` — it's available in
0.33 but blocks all input below; for the on-launch case that prevents
the user from getting to File → Open if they want to skip the wizard
and resume work on an existing project. Soft-modal (window on top,
underlying menus still clickable) is the right ergonomics.

**Auto-open at launch.** `wizard_open = true` in `App::new`. First-time
users see the wizard immediately; Cancel dismisses it and the rest of
the app behaves as before (the underlying state was the same
hardcoded "untitled 16×16" — nothing changes for users who hit Cancel
out of habit). File → New project re-opens it with fresh defaults
(`WizardState::default_for_new_project()`).

**Apply path.** `App::apply_wizard` calls `new_project()` first (which
already handles undo-history barrier + start-position clear + camera
reset), then writes `project_name` / `map_size` / `symmetry` /
`height_scale`, then runs `apply_procgen()` to materialise the biome's
expression. `apply_procgen` already calls `history.barrier()` so undo
state stays consistent. The procgen call is what populates the GPU
heightmap texture — after `apply_wizard` returns, the central rect is
showing terrain immediately.

**Alternatives considered:**
- **Inline wizard fields directly in the side panel.** Cheaper to ship,
  but conflicts with "this is a one-shot setup decision, not an
  always-visible control." Symmetry / biome live in the wizard *and* in
  the side panel — biome via the existing "Generate from formula"
  section (free-form), symmetry as a permanent sculpt control. The
  wizard is the curated subset for first-launch.
- **Skipping the rectangular refactor and only supporting it in the
  wizard.** Would leave the side-panel size control square-only, which
  is surprising once a user has loaded a rectangular project — they
  could see `16 × 18` in the heightmap dims but only edit `16`. Doing
  the refactor *with* the wizard, rather than after, keeps the model
  consistent.

**Consequence:**
- `App.map_size_smu: u32 → App.map_size: MapSize`. All 16 call sites
  refactored; clippy + tests green.
- `WizardState` (form fields) + `WizardAction { Apply, Cancel }`
  (one-frame outcome) in the app.
- `barme-core::project::sanitize_name` exposed publicly with 4 unit
  tests covering pass-through, disallowed-char collapse, edge trim, and
  filename safety against `/ \ : space`.
- `barme-core::procgen::BIOMES` table (4 presets) with a
  `max_height_hint` field. Existing `presets_all_parse_and_run` test
  extended to cover BIOMES too.
- Side panel now exposes both `smu_x` and `smu_z` DragValues.
- `FileAction::NewProject` renamed to `OpenWizard` — File → New project
  is now wizard-first.

## ADR-033 — Undo per-stroke copy-on-first-write (supersedes ADR-022 snapshot rule)

**Status:** Accepted (2026-05-18).
**Context:** ADR-022's per-stamp `StampSnapshot { rect, before }` model
captured the full bbox of every brush stamp emitted during a stroke.
For an LMB-down → LMB-up drag the engine emits one stamp per frame —
roughly 60–240 stamps for a multi-second sculpt — and each stamp's bbox
overlapped the previous frame's by ~95% at typical pointer speeds.
Capturing the same pixel 120 times bloated a single stroke to
~244 MB on a 16-SMU map at radius 1024, blowing past the 100 MB ring
cap by 2-3× on every paint pass. Logs showed runaway eviction of the
*previous* stroke whenever the current one finished, which destroyed
the multi-step undo affordance the system was supposed to provide.

The bug is structural — coalescing must happen at *pixel* granularity
within a stroke, not at *rect* granularity across stamps. A pixel's
pre-stroke value is captured exactly once (the first time any stamp
touches it). Subsequent stamps that overlap that pixel snapshot
nothing. The resulting committed entry covers the unioned bbox of all
pixels touched during the stroke — bounded by the heightmap size, not
by stamp count.

**Decision: copy-on-first-write within an open stroke.** `History`
owns the in-flight stroke state:
- A `Vec<u16>` mirroring heightmap dims exactly once (the scratch
  buffer).
- A packed `Vec<u64>` bitset, one bit per pixel, marking which slots in
  scratch hold a real pre-edit value.
Each frame, before the brush runs, the caller passes the union of
that frame's symmetric stamp rects to `History::snapshot_rect`. For
every pixel in the rect whose bit is clear, we copy from the heightmap
into scratch and set the bit. On `end_stroke`, we walk the bitset to
find the tight bbox of set bits, build a bbox-sized `Vec<u16>` (using
scratch for snapshotted pixels and the current heightmap value for
unsnapshotted pixels-in-bbox — those values match pre-stroke because
the stroke never touched them), and push that as a single `UndoEntry`.

**Memory bound:**
- Transient (while a stroke is open): `w·h·2` bytes for scratch +
  `w·h/8` bytes for the bitset (~2.1 MB at 16 SMU, ~4.5 MB at 32 SMU).
- Committed (per entry): `bbox.w · bbox.h · 2` bytes, capped at the
  full heightmap size (~2 MB at 16 SMU).

**Alternatives considered:**
- **`HashSet<(u32, u32)>` instead of a packed bitset.** Correct, but
  ~24 bytes per pixel — worse than the disease at 1025². Rejected.
- **Per-stamp rect union maintained on `History` instead of the
  bitset.** Avoids the heightmap-sized scratch buffer. Rejected
  because the bbox bound is then "union of input rects" rather than
  "union of pixels we actually snapshotted" — a caller that passes a
  rect that adds no novel pixels would still extend the bbox. The
  bitset gives a tighter, more predictable bound.
- **Reuse the scratch+bitset across strokes via a generation
  counter.** Avoids the per-stroke `vec![0u16; pixels]` allocation.
  Defer: profiling shows the alloc-and-drop pattern is sub-millisecond
  at 16 SMU and only happens on LMB-down. Reopen if 32 SMU exposes a
  hitch.
- **Replay strokes forward (command pattern).** Same rejection
  rationale as ADR-022 — kernels read pixels the previous stamp may
  have moved, so deterministic replay would have to remember every
  kernel application in order. Snapshots are simpler.

**Consequence:**
- `barme-core::undo` rewritten. `StampSnapshot` is gone; `UndoEntry`
  collapses to `{ rect: DirtyRect, before: Vec<u16> }`. New public API:
  `History::snapshot_rect(&Heightmap, DirtyRect)`,
  `History::end_stroke(&Heightmap)`, `History::stroke_open()`.
  `History::push(UndoEntry)` is gone — strokes commit themselves.
- `lib.rs` export `StampSnapshot` removed; `UndoEntry` retained but
  with the new fields.
- `App.stroke: Option<UndoEntry>` field removed — the in-flight stroke
  state lives inside `History` now.
- `App::end_stroke` becomes a thin wrapper that hands the current
  heightmap to `History::end_stroke`. The barrier path (procgen / load
  / new project) discards any in-flight stroke automatically.
- `History::barrier` now also drops the open stroke (previously a
  no-op for the `Option<UndoEntry>` on `App`, which was cleared
  separately).
- 12 unit tests on `undo` (previously 6): round-trip, overlapping
  stamps, 120-stamp-same-position bound, 120-stamp diagonal-drag
  bound, snapshot-then-undo byte-identity, redo invalidation, barrier
  drops open stroke, empty stroke no-op, cap eviction, off-rect-pixel
  correctness, bbox-of-set-bits exactness, redo chain.

## ADR-030 — Editor layout shell: five-zone panels + single-active-tool model

**Status:** Accepted (2026-05-18).
**Context:** The Phase-2 close shipped a working editor inside a single
280 px `SidePanel::left` stacked with eight unrelated sections —
project info, heightmap stats, render scale, tool mode, sculpt
controls, start-position list, procgen form, build & install. By
ADR-024 (F1 wizard) every Phase-3 feature is queued to plug into that
same column: symmetry as a "global mode" (B2), splat painting (D5),
metal/geo placement (C4 / C5), feature placement (C6), `mapinfo.lua`
form editor (C7), the linter panel (C8), and an enriched F8 allyteam
tree (B6). Without a layout refactor each one of those lands as
another section in the same scrolling pile — at six items the column
already needs vertical scroll on a 1080p display.

The two UX deep-research outputs (Claude + Gemini, both at
`docs/research/ui/`) converge on a five-zone shell: top action bar +
bottom status strip + left tool strip + right Inspector + central
viewport. They diverge on whether the left tool strip should sit
alongside a *second* "scene Outliner" left panel (Gemini's Unity
homage) or stay as a single narrow icon strip (Claude's Blender /
Krita / Photoshop pattern). We adopt **Claude's single-strip stance**
— Gemini's two-panel left has no strong UX precedent we found, and
egui's `SidePanel::left` resize handle on the second panel adds a
learning-curve cost without a clear win. If a real session of
B2/B3/B4 work proves the Inspector overflows with mixed
tool-params + project-metadata, the falsifier reopens this decision.

**Single-active-tool model.** The pre-ADR-030 `ToolMode { Sculpt,
StartPositions }` enum scaled poorly: every new editing surface
(Procgen, Splat, Metal, Geo, Feature, ...) would need a new variant
plus parallel state on `App`. Promoting `ToolMode` to `Tool { Select,
Sculpt, StartPositions, Procgen }` and driving the Inspector contents
via an exhaustive `match self.tool` builds the safety property
in: adding a `Tool::Splat` in Phase 4 produces a compile error at
every dispatch site, which is the failure mode we want. `Tool::ALL`
is the display-order array — the left tool strip iterates it, and a
unit test pins the size so new variants nudge the strip too.

**Q / B / S / G accelerators.** One letter per tool, gated on
`!ctx.wants_keyboard_input()` so typing into the procgen `TextEdit`
doesn't switch tools and eat the keystroke. Tool changes emit a
`tracing::info!` line with `from` / `to` so bug reports carry the
transition history. `App::previous_tool` is the sentinel — initialised
to `tool` so the first transition logs a real diff, not "??? → X".

**Panel add-order.** egui's documented rule
(https://docs.rs/egui/latest/egui/containers/panel/) is **top →
bottom → left(s) → right → CentralPanel LAST**. Reversing the order
means the CentralPanel eats the rect that a later panel was supposed
to claim. The new `App::update` body is a slim orchestrator that
calls one panel method per zone in that exact order, then drains a
`FileAction` queue, then renders the symmetry popover and the F1
wizard on top.

**Drag threshold = 8 px.** egui 0.33 exposes the click-vs-drag
threshold as `InputOptions::max_click_dist` (default 6 px — pointer
moves within that radius after press still register as clicks).
Bumping to 8 px restores the click-place vs drag-paint
disambiguation we need in `Tool::StartPositions` (single LMB-clicks
near an existing marker must not jiggle into a drag-paint). 8 px is
the Photoshop / Blender convention.

**Inspector header is global state.** Project name, map size,
heightmap dims, and max-height live in a persistent block at the top
of the right Inspector — always visible regardless of active tool.
They're session metadata, not tool parameters. Below the header, an
exhaustive `match self.tool` swaps the tool-specific controls. Each
branch is a private `App::inspector_*` method so they grow
independently when B2 / B6 / B7 / C4 / C5 / C6 / C7 each touch their
own tool.

**Symmetry chip as a popover.** Per ADR-019 symmetry replicates every
spatial edit, not just brush stamps — it's a session property, not a
Sculpt-section preference. The top-bar chip reads `Sym: <label>`;
clicking it toggles a small `egui::Window` carrying the existing
axis combo + rotational fold spinner. ADR-031 (B2) will replace the
popover with a canvas overlay (dashed axes + ghost brush rings); B1
just relocates the controls so they don't live inside the Sculpt
section any more (where they were unreachable in StartPositions /
Procgen modes).

**Top-bar Build button is a placeholder.** Plain `Button` at the
right edge of the top action bar (right-aligned via
`egui::Layout::right_to_left`). ADR-NNN (B4) styles it green +
adds the `Build / Build + Install / Build + Install + Launch`
variants `ComboBox`. The Side panel's "Build & Install" section is
gone — duplicate paths were a UX-research frustration.

**Status strip placeholders.** Bottom strip carries live
camera-orbit readout, project map dims, validation chip placeholder
("0 issues" — wired in C8), and a Build-state chip that mirrors
`last_install`. The "Camera readout" inside the old Sculpt section
is gone — it's session-global, not a tool parameter.

**Alternatives considered:**
- **Two-left-panel layout (Gemini's variant).** Reduced Inspector
  scrolling at the cost of an extra learning curve and an
  ambiguous "which left panel owns this control" question. Rejected;
  reopen if B2 / B6 / B7 prove the Inspector overflows.
- **Dock crate (`egui_dock`).** Adds a maintenance dependency for a
  one-off UI layout. Rejected — egui's primitive panel set covers
  the five-zone shell cleanly. Reopen only if multi-document or
  resizable-tab UX is needed.
- **Tool state in `ui.memory()` (egui's per-Id keyed store).**
  Survives tool switches at the cost of restart-loss for brush
  radius / procgen expression / F8 selection. Rejected — the user
  would lose their workflow state on every app restart, which is the
  "immediate-mode state ownership" pitfall called out in
  phase-3-plan.md B1. All tool-specific state stays on `App`.
- **Inline panel functions vs an `ui/` module dir.** The
  phase-3-plan called it a judgement call. Kept inline for B1 — the
  refactored `main.rs` grew from 1609 → ~2050 lines (about 27 %
  growth). If B2's symmetry overlay + B3's brush ring push past
  ~2500 lines, the next session can split `crates/barme-app/src/ui/`
  with one file per zone.

**Consequence:**
- `enum Tool { Select, Sculpt, StartPositions, Procgen }` replaces
  `ToolMode`. Carries `icon()` / `accel()` / `label()` helpers and a
  `Tool::ALL: [Tool; 4]` const used by the tool strip + tests.
- New `App` fields: `tool: Tool`, `previous_tool: Tool`,
  `symmetry_popover_open: bool`. `tool_mode: ToolMode` removed.
- `App::update` is now a 25-line orchestrator. The body splits into
  `App::handle_keyboard`, `App::top_bar`, `App::status_strip`,
  `App::tool_strip`, `App::inspector` (+ private
  `inspector_header / select / sculpt / start_positions / procgen`
  branches), `App::central`, `App::drain_action`,
  `App::symmetry_popover`. Each panel function takes `&mut self` +
  `&egui::Context` plus `&mut Option<FileAction>` where relevant.
- `App::set_tool` is the single mutation point: bumps
  `previous_tool`, emits `tracing::info!`, drops any in-flight
  heightmap stroke (`end_stroke`), clears any in-flight marker drag
  (`dragging_start_pos`).
- 8 unit tests added under `barme-app::tests`:
  `tool_all_array_has_unique_entries_per_variant`,
  `tool_helpers_are_distinct_per_variant`,
  `tool_accelerator_is_a_single_uppercase_letter`,
  `tool_accelerators_match_adr_030`,
  `set_tool_is_noop_when_new_matches_current`,
  `set_tool_updates_current_and_previous`,
  `set_tool_clears_in_flight_start_position_drag`,
  `fresh_app_has_consistent_previous_tool_sentinel`. Plus 3
  smoke tests pinning Phase 2 invariants
  (`b1_does_not_regress_procgen_apply_phase2`,
  `b1_does_not_regress_start_position_placement_phase2`,
  `b1_does_not_regress_undo_with_no_heightmap_phase2`).
  `barme-app` test count: 18 (was 7 after A4).
- The drag threshold is set once per frame at the top of
  `App::update` via `ctx.options_mut(|o| o.input_options.max_click_dist = 8.0)`.
  Idempotent; cheap.

## ADR-031 — Symmetry canvas overlay: dashed axes + mirror-brush ghosts

**Status:** Accepted (2026-05-18).
**Context:** ADR-019 (symmetry replicate engine) and ADR-030 (the
top-bar chip + popover) already give the user a way to *set* symmetry,
but at B1 close there's no visual confirmation of which axes are
active. UX research (`docs/research/ui/claude-research-findings.md`
§3 "Symmetry as a global mode" + §5 "On-canvas feedback") flagged
this as frustration #5 — the user paints, sees one stamp, can't tell
whether symmetry is on or which axis is mirroring. The Aseprite
convention (a draggable on-canvas symmetry handle) is the obvious
prior art for 2D editors; we adopt the visible-axis half but **lock
axes to the map's geometric centre** because the BAR engine assumes
centre-of-symmetry for spawn pairing (ADR-019). A movable handle
would let the user produce a project that mirrors visually but
breaks engine assumptions about start-position pairing — silent
corruption.

The brush ring (B3) is the cursor's primary feedback under Sculpt,
but it only shows where the *user* stamps. Symmetry replicates
through N derived centres — the user can't predict where those land
without a ghost ring at each image. Mirror-brush ghosts in B2 + the
primary ring in B3 together render the full prediction.

**Decision:**
- **Persistent canvas overlay.** Whenever `App.symmetry != None`, the
  central viewport renders symmetry guides on top of the wgpu terrain
  pass, before the start-position marker overlay:
  - Mirror modes (Horizontal / Vertical / Quad / DiagonalMain /
    DiagonalAnti): one or two dashed lines crossing the map through
    centre, end-to-end.
  - `Rotational { fold }`: `fold` dashed spokes from centre outwards,
    each clipped to the map rect's `[0, ex] × [0, ez]` bounds via a
    parametric ray-vs-rect intersection.
- **Dash cadence is in screen pixels** (8 on / 4 off), not world
  units. World-unit dashes shrink to a fuzzy solid under zoom-out
  (pitfall §B2.1). Below 32 px projected length the dash pattern
  ceases to read as dashed; we fall back to a thin solid line.
- **Mirror-brush ghosts.** When `Tool::Sculpt` is active AND a brush
  is selected AND `symmetry != None` AND the cursor is over the
  central rect, faint ghost rings render at every symmetry-derived
  centre (skipping the primary — B3 owns the primary ring). Ghost
  alpha is ~50 % of the primary's so the user can distinguish "where
  I click" from "where it lands by symmetry."
- **Ring radius in screen space.** Brush radius is in world elmos.
  We project the centre AND a tangent point `(cx + radius, 0, cz)`;
  the screen distance between the two projected points is the
  screen-space ring radius. Cheap, correct under perspective, no
  inverse-projection bookkeeping (pitfall §B2.4).
- **Reuse the existing y=0 raycast.** Cursor → world projection goes
  through `render::screen_to_world_y0` (already wired for brush
  placement in `apply_brush_at`). A second projection path is
  explicitly forbidden by pitfall §B2.5.
- **High-fold rotational crowding accepted.** For fold ≥ 8 the
  spokes pack densely near centre; we accept this rather than
  introduce an inner-circle fall-back. If a user complains about
  unreadability, the inner-circle variant lands in a follow-up.

**Module layout.** Overlay code lives in
`crates/barme-app/src/ui/overlay.rs`, with a sibling `mod.rs` and a
new `mod ui;` in `main.rs`. B1's closing log set the line-count
threshold for splitting at ~2500; B2 is on track to push past that
once B3's brush ring + nav gizmo + cheat-sheet land. Splitting now
keeps `App::central` from growing into a 500-line function.

**Alternatives considered:**
- **Movable axis handles (Aseprite convention).** Visually elegant
  but breaks ADR-019's geometric-centre assumption. Rejected; the
  engine's spawn-pair math would diverge from the editor preview.
- **Depth-conformant wgpu decal axes.** Render the dashed lines as
  a textured quad sampling the heightmap so the axis hugs the
  terrain surface. Deferred — Phase 4 polish. The 2D painter overlay
  reads "close enough" against the terrain and avoids a new shader.
- **Inner-circle fall-back for high-fold rotational.** Considered;
  deferred per the falsifier in
  `devlog/stage-1-ux-symmetry-global/theories.md` — accept the
  crowding until user feedback proves it unreadable.
- **Skip overlay when no heightmap is loaded.** Rejected — symmetry
  is a session property, not heightmap-derived; rendering the
  axes against the "Load a heightmap" placeholder still helps the
  user understand the active mode.

**Consequence:**
- New module `crates/barme-app/src/ui/overlay.rs` with
  `paint_symmetry_overlay`, `paint_brush_ghosts`, `brush_ring_color`,
  `BrushCursor`, plus pure helpers `axis_segments_for`,
  `dash_subsegments`, `ghost_centres`, `rotational_spoke_segments`,
  `clip_ray_to_rect`. The pure helpers are pulled out specifically so
  geometry / cadence / centre-derivation logic is unit-testable
  without a painter or wgpu context.
- `main.rs`: new `mod ui;` declaration; `App::central` calls the two
  paint functions after the wgpu callback and before the
  start-position marker block. `App` field-set unchanged.
- 36 new tests in `ui::overlay::tests` covering: `clip_ray_to_rect`
  (right/top/left/bottom/corner/inside-endpoint), spoke count +
  origin + endpoints per fold (2/3/4/6/8/12), spoke clipping on
  square + rectangular maps, `axis_segments_for` per `SymmetryAxis`
  variant + Rotational fold<2 vs fold≥2, `dash_subsegments`
  (zero-length / short-solid / long-dashed / threshold-boundary /
  diagonal direction), `ghost_centres` (None / H / V / Quad /
  DiagonalMain / Rotational per fold / map-centre degeneracy /
  off-map originating stamp), brush colour mapping (dominance +
  distinctness + neutral fallback), and a `BrushCursor` round-trip.
  `barme-app` test count: 54 (was 18 after B1).
- B3 inherits this module — primary brush ring + nav gizmo + first-
  launch hint + `?` cheat-sheet land in the same `ui/overlay.rs` file
  (or sibling files under `crates/barme-app/src/ui/`).
- Future B4 / B6 / B7 overlays drop into the same module.

## ADR-028 — Typed `mapinfo.lua` schema in `barme-core`

**Status:** Accepted (2026-05-18). Supersedes ADR-013's
"minimum-viable string formatter" language for the **data shape**;
the emitter half of ADR-013 stays in force until ADR-029 (C2) lands.

**Context:** Phase 3's research session
(`docs/research/mapinfo/claude-research-findings.md` +
`gemini-bar-map-metadata-research-findings.md`) catalogued the full
`mapinfo.lua` surface — 20+ top-level fields plus 12 named
sub-tables. The current emitter at `crates/barme-pipeline/src/mapinfo.rs`
fills < 10 % of the consumable surface. That's enough to *boot* a
map but not enough to ship one without the "featureless / untextured"
symptom, the "fog start equals fog end breaks build ETA" landmine,
or the "extractor radius = 500 breaks mex snap" footgun documented
in §7 of the digest.

Three downstream consumers all need the schema:
1. **C2 emitter** — three-file emission convention
   (`mapinfo.lua` + `mapconfig/map_metal_layout.lua` +
   `mapconfig/map_startboxes.lua` + `mapconfig/featureplacer/features.lua`)
   needs a typed source so each file knows what it owns.
2. **C7 F9 form editor** — every field needs a typed DragValue /
   TextEdit / Checkbox / color picker; without a schema the form
   is unmaintainable.
3. **C8 lint pass** — silent-failure detection (digest §7) walks
   the typed schema and flags issues like the splatDetailNormalTex
   ↔ specularTex pairing requirement.

Building the schema once and reusing it three times is the obvious
shape. ADR-013's hand-rolled string formatter served Stage 0's
"booting the engine"; it's not the right v1 for "edits any
mapinfo field." Schema first, emitter swap next (C2 / ADR-029),
form + lint after (C7 / C8).

**Decision:** new module `crates/barme-core/src/mapinfo_schema.rs`
holding `pub struct MapInfo` plus 9 sub-block structs (`SmfBlock`,
`LightingBlock`, `AtmosphereBlock`, `WaterBlock`, `SplatsBlock`,
`ResourcesBlock`, `TerrainTypeBlock`, `GrassBlock`, `TeamBlock` +
`SoundBlock`, `GuiBlock`). `MapInfo::bar_default()` populates BAR
conventions; `From<&Project> for MapInfo` reads project state on
top of the defaults.

**Pitfalls modelled at the type level** (digest §7 +
phase-3-plan.md C1 callouts):

- `lighting.sun_dir: [f32; 4]` — vec3 + sunStart distance, NOT
  `[f32; 3]`. Easy mistake; pinned by `bar_default_lighting_sun_dir_is_four_floats`.
- `TeamBlock` carries ONLY `start_pos`, NEVER `ally_team`. Both
  research reports confirm allyteam membership lives in
  `mapconfig/map_startboxes.lua` (separate file, C2's job). Pinned
  by exhaustive destructure in `team_block_carries_only_start_pos`.
- `extractor_radius: Some(80.0)` — BAR convention, NOT the engine
  default 500. Engine default breaks mex snap. Pinned by
  `bar_default_extractor_radius_is_80_not_engine_default_500`.
- `atmosphere.fog_start: Some(0.1)`, `fog_end: Some(1.0)` —
  distinct. Setting equal breaks the build-ETA grid renderer.
  Pinned by `bar_default_atmosphere_fog_is_not_equal`.
- `modtype: 3` — Chobby visibility gate. Pinned by
  `bar_default_modtype_is_3`.
- `depend` includes `"Map Helper v1"` — without it the engine
  serves fallback textures (the "untextured" symptom). Pinned by
  `bar_default_depend_includes_map_helper_v1`.
- `splats.tex_scales: [0.02; 4]`, `tex_mults: [1.0; 4]` — BAR
  defaults. Pinned.
- `splat_detail_normal_tex` paired with `specular_tex` — both
  modelled. C8 lint enforces.

**Project model addition:** `Project.mapinfo_overrides:
HashMap<String, toml::Value>` — F9 (C7) will populate this on
top of `MapInfo::bar_default()`. Carries unusual per-project
edits (custom skybox, dual-fog config, etc.) so the schema
doesn't need a bump for every gadget. Empty by default;
`#[serde(default, skip_serializing_if = "HashMap::is_empty")]`
so legacy projects load forward.

**Alternatives considered:**
- **Promote to a `barme-mapinfo` sub-crate now.** CLAUDE.md
  sketches this layout. Defer: today's schema is ~600 LOC + tests.
  Pull out into a sub-crate when it exceeds ~500 LOC of pure
  schema code (the form editor + lint will inflate it past that
  point).
- **Use `serde_json::Value` for `custom.*`.** Rejected: every
  other on-disk type in `Project` is TOML; mixing serde formats
  inside one struct surfaces awkwardly in the F9 form layer.
  `toml::Value` is consistent and round-trips cleanly through the
  project file.
- **Model every field as `Option<T>`.** Rejected for the truly
  required fields (`name`, `version`, `mapfile`, `modtype`, `smf`,
  `lighting`, `atmosphere`, `splats`, `resources`). Making them
  `Option` would let a load silently drop them and produce a
  pink-map / untextured failure at emission time. Required-by-design
  is enforced at the type level.

**Out of scope:**
- The Lua-text emitter — ADR-029 / C2.
- The F9 form editor — C7.
- The lint pass — C8.
- `Project.metal_spots`, `Project.geo_vents`, `Project.features`,
  `Project.ally_groups` — those land in C4 / C5 / C6 / B6. C1 only
  adds `mapinfo_overrides`.

**Consequence:**
- New module `crates/barme-core/src/mapinfo_schema.rs` (~600 LOC
  including tests).
- New `Project.mapinfo_overrides` field with serde-default load.
- `App.mapinfo_overrides` field mirrors `Project.mapinfo_overrides`
  so save / open round-trip preserves user edits across sessions
  even before F9 wires the editor surface.
- 21 unit tests pin every BAR-default value and every digest §7
  pitfall.
- ADR-013 STATUS UPDATE annotates the supersession scope
  (data-shape only; emitter unchanged until ADR-029).

## ADR-029 — Three-file emission convention + Lua AST emitter

**Status:** Accepted (2026-05-18). Supersedes the **emitter half** of
ADR-013 (the packaging half — system `7z`, `-ms=off`, `Solid = -`
check — stays in force). Sister to ADR-028 (data shape) and gated by
ADR-032 (B6 — `Project.ally_groups` data model that
`mapconfig/map_startboxes.lua` consumes).

**Context:** ADR-028 landed the typed `MapInfo` schema in
`barme-core`. The emitter at `barme-pipeline::mapinfo` was still
the ad-hoc string formatter from ADR-013 — ~10 % of the consumable
mapinfo surface, line-stitched with `format!`. The C7 form editor,
C8 linter, C4/C5/C6 placement tools, and the B6 allyteam UX all
need to round-trip through the schema → bytes path. Doing that with
`format!` per field is unsustainable and produces non-deterministic
key ordering (NFR-Audit / NFR-Determinism violations).

The research digest also surfaced a separate point: BAR maps don't
ship a single `mapinfo.lua`. They ship a **four-file convention**:

| File (archive path) | Role |
|---|---|
| `mapinfo.lua` | Engine config + lighting + resources + flat `teams[]` pool. |
| `mapconfig/map_metal_layout.lua` | Mex spots + geo vents (consumed by `resource_spot_finder` gadget). |
| `mapconfig/map_startboxes.lua` | Per-ally start-box polygons (consumed by SPADS + Chobby). |
| `mapconfig/featureplacer/features.lua` | Feature instances (consumed by feature-placer gadget). |

Without the three sidecars, SPADS auto-hosts default to whole-map
start boxes (terrible 8v8 matchups), mex snap and the F4 view fall
back to the SMF metalmap (suboptimal spot clustering), and the
visual steam plumes for geo vents are missing. These are silent
failures the engine doesn't crash on — they only surface at
ranked-play / community-review time.

**Decision:**
- **New module `barme-pipeline::lua_ast`** carries `LuaKey`,
  `LuaValue`, and `serialize(&LuaValue) -> String`. 2-space indent,
  trailing commas, identifier vs `["bracketed"]` key forms, full
  escape coverage (`\\`, `\"`, `\n`, `\r`, `\t`), `f32`-aware float
  formatting (avoids the `0.1f32 as f64 → "0.10000000149011612"`
  widening that bare `{:?}` would produce). `sort_table_by_key` is
  the deterministic-emission helper every per-block builder calls
  before handing its table off.
- **`barme-pipeline::mapinfo` rewritten** on top of the AST. Walks
  the typed schema from ADR-028 (`MapInfo::from(&Project)`) and
  builds one `LuaValue` per sub-block. Empty `teams[]` falls back
  to the 25 % / 75 % diagonal pair (preserves ADR-013 behaviour for
  pre-F8 projects).
- **Three new sibling modules**: `metal_layout.rs`, `startboxes.rs`,
  `featureplacer.rs`. Each exposes `render(&Project) -> String`
  emitting `return { … }` text. This sprint they emit **empty
  placeholder bodies** — the data sources (`Project.metal_spots`,
  `ally_groups[*].box_polygon`, `Project.features`) land in C4 /
  C5 / B6 / C6.
- **`build_sd7` stages all four files** into the SD7 archive at
  their canonical paths: `mapinfo.lua` at root,
  `mapconfig/map_metal_layout.lua`,
  `mapconfig/map_startboxes.lua`,
  `mapconfig/featureplacer/features.lua`. PITFALL #2:
  `featureplacer/` lives at archive root, NOT inside `LuaGaia/`.
- **Key naming.** Rust snake_case → BAR Lua camelCase. The mapping
  is explicit in the per-block builders, no auto-conversion (the
  BAR community style guide has historical exceptions:
  `maphardness` is lowercase, `smtFileName0` is camelCase with a
  trailing digit). The mapping is implicitly tested via
  golden-substring assertions.
- **Determinism.** Every keyed table is sorted alphabetically by
  rendered key before emission. Integer-keyed tables sort
  numerically. Sequence tables preserve input order (caller's
  responsibility; e.g. `teams[]` is built in increasing index
  order). Repeated `render()` calls produce byte-identical output —
  pinned by `determinism_repeated_render_byte_identical` in each
  emitter module.
- **`description` escape coverage.** A field as benign as
  `description = "Has \"quotes\" and\nnewlines"` round-trips through
  the emitter without breaking the Lua parser. Pinned by
  `description_with_quotes_and_newlines_escapes` in the mapinfo
  tests and by the AST's own escape-coverage tests.

**Pitfalls pinned at compile / test time:**
- `teams[]` carries ONLY `startPos`. The C1 schema's `TeamBlock`
  exhaustive-destructure test (`team_block_carries_only_start_pos`)
  prevents anyone re-adding `allyTeam`. Emission walks this shape
  unchanged.
- `mapinfo.depend` includes `"Map Helper v1"`. `bar_default` does
  the work; the emitter test
  `depend_contains_map_helper_v1` pins the round-trip.
- `extractorRadius = 80.0`, `fogStart = 0.1`, `fogEnd = 1.0`. All
  pinned in `mapinfo::tests` by string-substring assertions.
- `featureplacer/features.lua` lives at `mapconfig/featureplacer/`,
  not `LuaGaia/`. Pinned by the `archive_rel` literal in
  `build_sd7`. (Gemini's report misspelled this path; Claude's
  was correct — adopted Claude per phase-3-plan §"Adopt Claude on
  every divergence".)
- Feature `rot` is **string-typed** Spring heading (`"0"`…`"65535"`),
  not float. Pinned by
  `featureplacer::tests::feature_entry_carries_name_x_z_rot_as_string`.
- `map_startboxes.lua` is empty when `ally_groups.len() < 2`.
  Pinned by `startboxes::tests::empty_when_under_two_ally_groups`.

**Alternatives considered:**
- **Keep growing the string formatter.** Rejected — every new
  field doubles the surface, and string concat can't enforce
  determinism. Sub-block separation is already painful at the
  current ~30-field surface; the schema's 100+ fields would
  collapse the format strings under their own weight.
- **Promote to a separate `barme-mapinfo` crate now.** CLAUDE.md's
  repo-layout sketch reserves the name. Defer until the schema +
  emitter + sidecars exceed ~700 combined LOC of non-test code.
  Current total (lua_ast + mapinfo + 3 sidecars) is well under that;
  4 sibling modules under `barme-pipeline/src/` are right-sized.
- **`lua-rs` / `mlua` AST.** Rejected: those are full-fidelity Lua
  parsers / evaluators. The emitter only needs to *write*; not
  read. A 200-line AST is the appropriately scoped tool.
- **JSON serializer + Lua post-process.** Rejected: the BAR
  community reads mapinfo by hand. Round-tripping through JSON
  loses the human-friendly idioms (trailing commas, integer-keyed
  table form) the community style guide expects.

**Out of scope (later items):**
- Real bodies for the three sidecars: C4 (metal), C5 (geo), C6
  (features). C2 ships placeholders.
- F9 form editor — C7. The schema is editable; this commit makes it
  *emittable*.
- Lint pass — C8. The emitter doesn't validate; it just renders.
- The `barme-mapinfo` sub-crate split — deferred until LOC justifies.

**Consequence:**
- 4 new files in `crates/barme-pipeline/src/`: `lua_ast.rs`,
  `metal_layout.rs`, `startboxes.rs`, `featureplacer.rs`.
- `mapinfo.rs` rewritten (~430 LOC including tests).
- `lib.rs` re-exports + `build_sd7` grows to stage 4 Lua files
  alongside the SMF/SMT.
- `barme-pipeline` test count: 31 → 47 (+16 new across AST + 3
  sidecars + new emitter coverage).
- ADR-013 status-updated to scope its supersession (emitter half
  superseded; packaging half retained).
- `Project.ally_groups` does not yet exist — `startboxes::render`
  reads via a `ally_group_boxes` helper that returns `Vec::new()`
  until B6 lands. C2 + B6 are bundled this sprint for exactly this
  reason: B6 swaps the data source without changing the emitter
  shape.

## ADR-032 — Start-position allyteam redesign (B6)

**Status:** Accepted (2026-05-18). Supersedes ADR-023's flat
`start_positions` data shape. Sister to ADR-029 (C2 emitter consumes
the new tree).

**Context:** ADR-023 shipped a flat `Vec<StartPosition>` with a
single team per symmetry image. That's correct for 1v1 maps but wrong
for everything BAR actually plays in queue: 8v8 wants 16 positions
across 2 sides (not 1 per side), 3-way FFA wants 3 distinct sides
each with their own start box, 4-way FFA wants 4 corners. The Phase-3
research session (`docs/research/ui/claude-research-findings.md` §4)
catalogues the failure mode from real BAR community feedback:

> "the start positions defined from mapinfo.lua are a plain list,
> which is not flexible enough to define multiple layouts of start
> positions depending on the number of contestants" — BAR FFA gadget
> README.

The fix isn't a richer flat list — it's a **two-level tree** that
makes ally-team membership a first-class concept in the editor.

**Decision:** introduce `AllyGroup` carrying its own colour, name,
source start positions, and an optional `box_polygon` that drives the
sibling `mapconfig/map_startboxes.lua` emission (ADR-029 / C2).
`Project.ally_groups: Vec<AllyGroup>` replaces
`Project.start_positions: Vec<StartPosition>`. `StartPosition` is
stripped to `{ x_elmo: i32, z_elmo: i32 }` — team identity is now
positional (computed at emission time from
`ally_groups[*].start_positions` walked in id order).

**Mirror placement strategy.** Symmetry-replicated mirrors go into
the **same ally group** as the source. A Quad-symmetric placement on
group 0 produces 4 positions in group 0, not 4 separate groups. The
research callout: "derived positions go into THE SAME ally group …
Derived positions render greyed in the tree". Sources are stored;
mirrors are recomputed every frame from the active symmetry axis.
Trade-off: toggling symmetry off mid-session "forgets" the mirrored
positions visually, but the BUILD path expands sources through the
active symmetry into the same group before passing Project to the
pipeline emitter, so the `.sd7` always ships every spawn the user
saw on canvas.

**Backwards compatibility.** Pre-Phase-3 `.barmeproj` files load via
a custom `Deserialize` on Project. The wire-format struct
(`ProjectWire`) accepts both `[[ally_groups]]` (new) and
`[[start_positions]]` (legacy). When only the legacy field is
present, the migration materialises every position into
`ally_groups[0]` with the default colour + name `"AllyGroup 0"`. The
legacy `team_id` field is read by serde but ignored — team ids are
positional now. A fixture test pins the migration:
`legacy_flat_start_positions_load_into_ally_group_zero`.

**Inspector tree.** One `CollapsingHeader` per ally group, with:
- A colour swatch keyed off `egui::Id::new(("ally_group_header",
  group.id))` — persistent across tool switches + tree rebuilds.
  Without this `Id` keying, `color_edit_button_srgba`'s popover loses
  state every frame the tree is rebuilt (PITFALL — egui retains
  popover state by widget Id).
- Name `TextEdit`, position count, ★ active-group toggle, delete.
- Child rows for source positions (index, coords, ×).
- Greyed-out rows for symmetry-derived mirror positions, with a
  `(mirror of #N)` label and a tooltip that points the user back at
  the source — derived positions are NOT separately editable.
- "+ Add AllyGroup" at the bottom.

A configuration preset dropdown above the tree applies one of:
`1v1` (2 groups × 1 pos each), `8v8` (2 groups × 8 pos, north/south
strips), `3-way FFA` (3 groups in a triangle), `4-way FFA` (4
corners). Each preset also populates the per-group `box_polygon`
matching the BAR community 8v8 convention (north strip = `[(0, 0),
(1, 0.12)]`; south strip = `[(0, 0.88), (1, 1)]`; etc.). The
emitter consumes those polygons directly — no second authoring path.

**Canvas interaction.** LMB-click in empty terrain places one
position in the active ally group; LMB-drag distributes N
evenly-spaced positions along the drag vector (N defaults to 8 — the
canonical 8v8 case, configurable in the Inspector). LMB-drag on an
existing marker moves it. RMB-click on a marker deletes. The
single-click vs drag-paint disambiguator depends on B1's 8 px drag
threshold (`InputOptions::max_click_dist = 8.0`); N=1 single click
doesn't fire the drag-paint branch because the drag-stop handler
threshold-checks the line length before committing.

**Hover↔pulse feedback.** Hover an Inspector row → marker pulses at
2 Hz for 1 s after the hover instant (`pulsing_marker` field on App
carries `(id, idx, Instant)`; the marker draw loop modulates radius
by `(dt * 2π * 2.0).sin().abs()` and `ctx.request_repaint()`s until
the second elapses). Hover a marker on canvas → Inspector
auto-scrolls to the matching row via `Response::scroll_to_me`.

**Cross-tool ghosting.** Markers render at 50 % alpha when the
StartPositions tool is not active, and don't respond to hover. Same
B1 convention as the symmetry overlay's "Sculpt-only ghosts".

**Undo (B5 / ADR-033 integration).** `ProjectDiff` variants change
shape to identify positions by `(ally_group_id, pos)` instead of
`team_id`. The new variants are
`PlaceStartPosition { ally_group_id, pos }`,
`DeleteStartPosition { ally_group_id, pos }`,
`MoveStartPosition { ally_group_id, from, to }`. The wizard snapshot
now carries `ally_groups: Vec<AllyGroup>` so Ctrl-Z over an F1
wizard apply restores the full tree (colours, names, polygons).

**Build path.** `App::snapshot_project_for_build()` clones the
project, calls `expand_symmetry_into_ally_groups(&mut p, symmetry)`
which iterates each source position and re-materialises its mirrors
into the same group (deduped by exact coords), then passes the
expanded Project to the pipeline. This is the SINGLE point where
sources expand into the flat `teams[]` pool the engine consumes.
Without this expansion, a Quad-symmetric placement would ship 1
team to BAR even though the canvas showed 4 markers.

**Out of scope:**
- Per-position colour override — defer; AllyGroup.color is enough.
- Symmetry-grouped drag (move all mirrors as one) — Phase 4.
- Box-polygon editor UI — emission only this sprint; presets supply
  the polygons. Manual editing waits for a polygon-editor tool.
- F12 launch-with-this-spawn debug.

**Consequence:**
- `barme-core::project` rewrites: `AllyGroup` struct,
  `ALLY_GROUP_PALETTE` constant, `ProjectWire` legacy-migration
  shim. ~330 LOC including tests.
- `barme-core::undo`: ProjectDiff variants restructured; tests
  updated to operate on `AllyGroup` instead of a bare
  `Vec<StartPosition>`. Same line-count.
- `barme-core::mapinfo_schema::From<&Project>` walks `ally_groups`
  in id order and flattens into `teams[]`.
- `barme-pipeline::startboxes` repoints its `ally_group_boxes`
  helper at `project.ally_groups[*].box_polygon`.
- `barme-app`: the F8 Inspector rewrite + canvas interaction
  changes + ~+10 fields on App. ~+500 LOC including tests.
- `team_color()` palette helper deleted (per-AllyGroup colour
  replaces it).
- 7 new app-side tests cover drag-paint, preset materialisation,
  symmetry-expansion-at-build, and the migration path. Existing F8
  smokes (`b1_does_not_regress_start_position_placement_phase2`,
  `b5_*`) refactored against the new shape.
- ADR-023 status-updated to scope its supersession (data shape
  superseded; UX surface remains but is rebuilt around the tree).

## ADR-025 — Starter texture pack: 16 ambientCG CC0 slots in 4 biome groups

**Status:** Accepted (2026-05-18)

**Context:** F4 (splat painting) needs a populated tile palette to be
usable. We do not want first-launch to require a scavenger hunt through
the Nobiax/Beherith DNTS pack, World Machine licences, or per-author
ambientCG downloads. Two research sessions produced competing palette
sketches (Claude — `docs/research/textures/claude-findings-from-research.md`
— ambientCG-only, 16 slots, synthesise-normals-from-luminance; Gemini —
`docs/research/textures/Gemini BAR Editor Texture Pack Scoping.md` — 16
slots, 4 biome groups, mixed ambientCG + Poly Haven, bundle-the-source-
normal-map). Sprint 7's brief reconciles them: take Gemini's biome
structure, take Gemini's bundle-the-normal stance, but route every slot
through ambientCG (no Poly Haven). Poly Haven was excluded because
(a) per-asset licences vary — not every Poly Haven asset is CC0; (b) 1K
ZIPs run 77–99 MB per slot due to GL+DX normal pairs + 16-bit
displacement, blowing out the per-slot disk budget; (c) Poly Haven's
direct-file UX doesn't map cleanly to the `_1K-<JPG|PNG>.zip` URL
pattern this script targets. The source audit at
`docs/research/source-audit-2026-05-18/FINDINGS.md` §7 corrects
splat-rendering math and informs the format choices below.

**Decision:**
- **16 slots, 4 biome groups × 4 textures.** Biome groups are
  `Earth-Temperate`, `Arid`, `Snow-Alpine`, `Alien-Industrial`. Names
  are short kebab-case strings (`grass-meadow`, `clean-metal-floor`).
- **Sources are 100 % ambientCG, all CC0-1.0.** Verified per-asset URL
  status, content, and sha256 on 2026-05-18; pinned in
  `scripts/fetch-textures.sh`. Per-asset attribution is not required
  under CC0; `CREDITS.md` credits ambientCG and Beherith as a courtesy.
- **Pinned 16-slot palette table** (sha256 over the `_1K-PNG.zip`):

| # | Name | Biome | ambientCG asset | sha256 |
|--:|---|---|---|---|
| 00 | `grass-meadow` | Earth-Temperate | `Grass002` | `3a51690e1fd2fd6672f8964737091eb52444c1ed90f58f16bf79a50d2e5aa517` |
| 01 | `forest-floor-pine` | Earth-Temperate | `Ground037` | `cbd75f0660870b3299a68c4fe7fd54efb3951cf992d4295961209619eb284c47` |
| 02 | `dirt-mud-cracked` | Earth-Temperate | `Ground042` | `2a2e34f68981519f81a6b8cc982a68b51669185971cefa038d8055a02d7c7443` |
| 03 | `rocky-outcrop-grey` | Earth-Temperate | `Rock030` | `d3e0dc55fc46b093631f4d0009c934003c601e69df9aa4ba41a43db3807056ee` |
| 04 | `desert-sand-dunes` | Arid | `Ground027` | `03e41b00d17ed28c235cccaa6aba74015b49961e2fe657c75c59b55ccf8fd050` |
| 05 | `dry-rock-sandstone` | Arid | `Rock023` | `4d6a7d7a36bf6dfbe4fe456cc748bd875c2bb95c3135aff0785f86928ea3b0d2` |
| 06 | `dusty-hardpan-clay` | Arid | `Ground033` | `b8dbd0105b204863b9b1b6d9e2656fa4f7f77398eebfdef644349140b6da3a72` |
| 07 | `arid-gravel-pebbles` | Arid | `Gravel018` | `aceb088008927d82085629a7d765abb4c2d704fdbbf5d185669757c4bdfd9616` |
| 08 | `alpine-snow-powder` | Snow-Alpine | `Snow004` | `ed08bcfdcc0a57e815dba6fc64429d7498773be41381fdf35efc5b771e286472` |
| 09 | `jagged-ice-frozen` | Snow-Alpine | `Snow006` | `f993019a7e2a59bfdf3ddeb9b4e692bc2a734a5fb71f17581234f24065dbdac5` |
| 10 | `cold-bare-rock` | Snow-Alpine | `Rock029` | `b8d3517cc73bf317a32ad1c3ca8bb4e4c7b8aed0eab30ee24a00c623374a8764` |
| 11 | `frozen-permafrost` | Snow-Alpine | `Ground035` | `ed88469c201a41f82776d8651d947a0ea00a9412fca7a2261aa79dc162ffb257` |
| 12 | `dark-volcanic-lava` | Alien-Industrial | `Rock035` | `e745b558d754962ac44162ccee8805d7dba84ecdc428a543c3e552bcb28f8b85` |
| 13 | `rusty-metal-plates` | Alien-Industrial | `Metal009` | `ec44086a3bee042418ac2b38a74c8cedfa8313d942bf08c1c91be4ef63c8a97f` |
| 14 | `clean-metal-floor` | Alien-Industrial | `Metal003` | `b664c3a54bb5e5fc879bb0f69f0f51e2bfd7925c014ca076c779912a72ef2e50` |
| 15 | `alien-organic-creep` | Alien-Industrial | `Moss001` | `e3745c52f895acf88ce3f28fa83aebc0b7371b68378022f813dcc16ffb0aa8c8` |

- **Format choice — `_1K-PNG.zip`, not `_1K-JPG.zip`.** The JPG-variant
  ZIPs ship JPG-encoded normals (`*_NormalGL.jpg`). JPEG's 4:2:0 chroma
  subsampling destroys X/Y vector data in tangent-space normals — this
  is `docs/PITFALLS.md` rule #2. The PNG-variant ZIPs ship lossless
  PNG normals at ~6 MB each. We accept the larger network footprint
  (~16 MB/zip × 16 ≈ 256 MB) for vector-correct normals. Diffuse is
  also extracted as PNG for the same ZIP, which is slightly larger
  than JPG but avoids a second download.
- **Normal-map convention — `_NormalGL` (OpenGL).** Recoil's SMF
  fragment shader builds the TBN from the per-vertex normal and decodes
  per-splat normals as `* 2 - 1` (FINDINGS §7.4); the tangent space is
  OpenGL (+Y up). ambientCG `_NormalGL` is the matching convention.
  **No Y-flip needed at fetch time.** D2's bake pipeline (ADR-026)
  surfaces an opt-in Y-flip step for future user-imported DirectX-source
  normals (F23 / Phase 6); the starter pack flows through with the
  flip disabled.
- **Bundle source normal maps (Gemini), don't synthesise them (Claude).**
  Sobel-from-luminance produces visibly wrong tangent-space data on
  assets authored with specific micro-relief (brushed metal,
  cracked clay). Per-slot disk cost is ~8 MB (2 MB diffuse + 6 MB
  normal); total `tools/textures/` ≈ 115 MB. `tools/textures/` is
  gitignored (`.gitignore` updated this sprint).
- **`splatDetailNormalDiffuseAlpha = false` baseline.** Per FINDINGS
  §7.3 the engine decodes the entire RGBA sample as signed
  (`* 2 - 1`) and multiplies by `splatCofac`. With `alpha = false`,
  the alpha channel ships at `0xFF` in every DNTS DDS, contributing
  nothing observable to the composite. The `true` workflow (alpha
  carries a signed high-pass diffuse offset) is the visually-richer
  path but easy to get wrong; deferred to ADR-034 once D4 lands the
  splat preview shader and we can A/B in-engine. **The fetch script
  does not bake the high-pass — that lives in D2 (ADR-026).**
- **`default_tex_scale = 0.02`.** Engine-historical default per
  `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl:25`
  (`#define SMF_DETAILTEX_RES 0.02`) and Beherith's DNTS announcement
  (`https://springrts.com/phpbb/viewtopic.php?t=22564`: "SplatTexScales
  defaults to (0.02 0.02 0.02 0.02)…lower values mean larger size").
  Real-world BAR maps run smaller per-channel —
  `scratch/bar-maps/extracted/comet/mapinfo.lua:116` ships
  `texScales = {0.004, 0.007, 0.003, 0.0018}` for Comet Catcher Remake.
  0.02 stays as the meta.toml default for new-user predictability;
  the D5 splat-tool UI (Sprint 9) surfaces the real-world range
  `0.0015..=0.05` as the slider span with a tooltip explaining the
  smaller per-channel norm. `default_tex_mult = 1.0` is the equivalent
  engine baseline.

**Alternatives:**
- **Adopt Gemini's table verbatim (mixed ambientCG + Poly Haven).**
  Rejected per per-asset licence variance, 77–99 MB Poly Haven zips,
  and the awkward Poly Haven download UX.
- **Fork the Nobiax/Beherith DNTS pack** (CC0; engine-native). The
  pack ships pre-baked DNTS DDS (normal-in-RGB, diffuse-in-alpha)
  which the editor cannot preview as diffuse without an unbake step.
  Bundling raw CC0 sources is strictly more flexible and yields the
  same DNTS output via D2's bake pipeline.
- **Synthesise normals from luminance** (Claude's stance — Sobel on
  diffuse). Rejected per the Gemini analysis: assets with deliberate
  micro-relief (brushed metal, cracked clay) Sobel into visually-
  wrong tangent vectors. Disk cost of bundling source normals is
  bounded at ~6 MB/slot and gitignored anyway.
- **First-launch downloader.** Rejected for the starter experience —
  breaks offline first-run and adds a network-failure UX path. The
  user-import path (F23 / Phase 6) is the polish path for arbitrary
  additional slots.
- **OpenGameArt-sourced textures.** Rejected — mixed CC-BY / CC-BY-SA
  licences would contaminate the user's `.sd7` with attribution and
  share-alike obligations.
- **JPG-variant ambientCG zips.** Rejected per pitfall #2 (JPG normals).

**Substitutions vs Gemini's research** (recorded for traceability):
- **Slot 3 / 5 / 9 / 13** routed off Poly Haven onto ambientCG per
  Sprint 7's brief. New IDs: `Rock030`, `Rock023`, `Snow006`,
  `Metal009`.
- **Slot 4** (`desert-sand-dunes`): Gemini's `Sand002` is a
  hallucination — `Sand001-010` all 404 on ambientCG (sand textures
  live under the `Ground` prefix in ambientCG's taxonomy). Replaced
  with `Ground027` (Claude's slot 5 "sand-dune-fine").
- **Slot 10** (`cold-bare-rock`): Gemini routed both slot 3 and slot
  10 to `Rock030`. After substituting slot 3 to `Rock030`, slot 10
  collides. Replaced with `Rock029` (adjacent in numbering; same
  cool-grey weathered-cliff visual register).
- **Slot 14** (`clean-metal-floor`): Gemini's `Metal042` is a
  hallucination. Replaced with `Metal003` (clean industrial diamond
  plate; canonical low-numbered Metal entry).
- **Slot 15** (`alien-organic-creep`): Gemini's `Organic001` is a
  hallucination. Replaced with `Moss001` (purpose-built moss texture;
  matches "organic/lichenoid surface" intent).

**Consequence:**
- New file `scripts/fetch-textures.sh`. Idempotent (second run is a
  ~30 ms stat sweep); `--check` mode HEAD-probes each URL without
  downloading (use this in CI to detect ambientCG asset rot). Bootstrap
  workflow: replace any slot's sha with the literal `BOOTSTRAP` to make
  the script print the computed sha + exit non-zero; paste the value
  back in to re-pin.
- New file `CREDITS.md` at repo root. Single CC0 banner + per-asset
  attribution table + Beherith courtesy line.
- `.gitignore` grows `/tools/textures/`.
- Per-slot directory layout is the contract D3 (`barme-core::splat`
  registry) depends on; locked in **ADR-027** below.
- `cargo` workspace unaffected — this sprint touches no Rust.
- The starter pack's `splatDetailNormalDiffuseAlpha = false` baseline
  is the contract D2's bake honours. The high-pass-diffuse-in-alpha
  workflow lives behind ADR-034.

## ADR-026 — DNTS bake pipeline: `splatDetailNormalTex` BC3 emit with sha256 cache

**Status:** Accepted (2026-05-18)

**Context:** F4 (splat painting) needs a `splatDetailNormalTex`-format
DDS per slot at `.sd7` build time. The starter pack (ADR-025) ships
raw diffuse + normal PNGs; the bake step composes them into a single
RGBA8 image and BC3-compresses to DDS. Up to 16 slots × N builds
amounts to a lot of repeat compression — a content-addressed cache
keeps incremental builds fast.

The shader-math facts that constrain the bake:
- The engine builds the TBN from the per-vertex normal and decodes
  splat normals as `* 2 - 1` (FINDINGS §7.4, `SMFFragProg.glsl:174-198`).
  Y-flip must match source convention — ambientCG `*_NormalGL.png` is
  already OpenGL; Substance / Quixel exports are DirectX-source. Both
  paths need to round-trip.
- The full RGBA of a DNTS sample is decoded as signed; alpha
  contributes to the per-pixel diffuse offset
  (`splatDetailStrength.y = clamp(splatDetailNormal.a, -1, 1)` when
  `SMF_DETAIL_NORMAL_DIFFUSE_ALPHA` is defined; otherwise alpha is
  unused). With `splatDetailNormalDiffuseAlpha = false` (ADR-025's
  baseline) the alpha can be solid 0xFF without changing the
  rendered result.
- BC3 carries 8-bit alpha; BC1 doesn't. We pick BC3 unconditionally
  so the upgrade path to ADR-034's high-pass alpha workflow stays
  open without re-baking the BCn format.

**Decision:**
- New module `crates/barme-pipeline/src/dnts.rs`. Public surface:
  ```rust
  pub struct BakeOptions {
      pub yflip_normal: bool,    // default false; starter pack is _NormalGL
      pub diffuse_in_alpha: bool, // default false; ADR-025 baseline
  }
  pub fn bake_dnts(slot_dir: &Path, out_dds: &Path, opts: BakeOptions) -> Result<()>;
  ```
- **Y-flip is a runtime knob**, default OFF. The D1-shipped starter pack
  ships ambientCG `*_NormalGL.png` (OpenGL convention) → no flip needed.
  F23 user-imports of DirectX-source normals (Substance / Quixel) flip
  the G channel via `255 - g`. A unit test pins a synthetic normal map
  through both branches.
- **JPG normals rejected at the entry point.** PITFALLS rule #2 — JPEG
  chroma subsampling destroys X/Y vector data. A `normal.jpg` present
  without `normal.png` returns a typed error (`NormalNotPng`).
- **Compose RGBA8**: RGB = (possibly flipped) normal RGB; A = 0xFF
  when `diffuse_in_alpha == false` (ADR-025 baseline), else the
  Rec.709 luma of the diffuse pixel. The luma path ships untested in
  BAR — high-pass tuning is ADR-034.
- **BC3 / DXT5 always** via the vendored `compressonatorcli-bin`
  (ADR-014). The wrapper shell script ships as `CompressonatorCLI` →
  `compressonatorcli`; we invoke the underlying ELF directly with
  `LD_LIBRARY_PATH` set to the wrapper's exact entries
  (`compressonator/`, `compressonator/qt/`, `compressonator/pkglibs/`).
  Direct ELF invocation avoids a fork-exec ENOEXEC the Rust subprocess
  path hits on the bash wrapper inside `cargo test`'s harness. The
  `CompressonatorCLI` symlink is kept as the fetch-script-ran canary.
- **Subprocess pattern** mirrors ADR-012 (PyMapConv driver): capture
  stdout + stderr, stream both to `tracing::trace!`, trust artifact
  presence as the success contract (warn-and-accept on non-zero exit
  if the DDS landed).
- **Cache** lives at `tools/textures-cache/<sha>.dds`. The cache key
  is `sha256(diffuse_bytes ‖ normal_bytes ‖ opts.to_cache_bytes())`.
  Identical inputs → cache hit → copy. Different bytes OR different
  opts → cache miss → bake. `.gitignore` grows `/tools/textures-cache/`.
- **Compressonator flags**: `-fd BC3 -nomipmap` plus the input PNG +
  output DDS paths. No mip generation (mipmaps come downstream when
  the .sd7 is loaded; the DDS itself only needs base-level data).
- **Discovery**: `bake_dnts` walks up two parents from `slot_dir`
  (typically `tools/textures/<NN-slot>/`) to find `tools/`, then
  resolves `tools/compressonator/compressonatorcli-bin` and
  `tools/textures-cache/`. Tests use an internal `BakeEnv` struct
  to redirect both paths for hermetic runs.

**Alternatives:**
- **BC1 when `diffuse_in_alpha == false`.** Saves ~50 % per-DDS disk
  vs BC3. Rejected: would re-bake every slot the day ADR-034 lands,
  and BC3 at 1024² is ~1 MB compressed — well under the 50 MB
  total budget the texture pack pre-paid.
- **Bake mipmaps.** Compressonator can generate them with `-miplevels`.
  Rejected: the engine generates its own mip chain at load time per
  the SMF tile format; an extra in-DDS mip chain just inflates the
  archive.
- **Invoke the `compressonatorcli` bash wrapper directly.** Hits
  ENOEXEC ("Exec format error") under `cargo test`'s subprocess
  spawn — the test harness rejects the kernel-level shebang
  interpretation for the wrapper script. Workaround: invoke the
  ELF directly with the wrapper's LD_LIBRARY_PATH set.
- **Synthesise normals from diffuse luminance (Sobel).** Rejected at
  ADR-025: visibly wrong on assets with deliberate micro-relief
  (brushed metal, cracked clay). The starter pack ships source
  normals; the bake just reformats them.
- **Cache key on file mtime instead of content.** Rejected: file
  systems with second-resolution mtime would false-positive a cache
  hit on edits within the same second. Content-addressed sha256 is
  the standard choice and the cost (a single 1024² PNG hash) is
  negligible against a BC3 compress.
- **Per-build temp dir instead of `tools/textures-cache/`.** Rejected:
  the cache persists across builds, across project switches, and
  across `git clean` (it's gitignored, not tracked, but stays through
  normal dev). The disk cost is bounded at `slots × BakeOptions
  variants × 1 MB`.

**Consequence:**
- `barme-pipeline` gains a new `sha2` dep (workspace-level).
- `.gitignore` lists `/tools/textures-cache/`.
- D6 (Sprint 12) consumes `bake_dnts` from the build orchestrator to
  produce `<slot>_dnts.dds` per active splat slot.
- Future D5 splat-tool-UI previews can reuse the same composed
  RGBA8 (without the BC3 step) — the helper is internal but
  promotable.
- The bake is reusable for F23 user-imports: same API surface,
  flip the `yflip_normal` flag when the user identifies a DirectX
  source. The cache key folds `BakeOptions`, so toggling the flip
  on the same source bytes is a clean re-bake.
- `diffuse_in_alpha = true` is plumbed but UNTESTED in BAR; the
  in-engine A/B that confirms the high-pass path is ADR-034.

## ADR-027 — Asset registry on-disk layout (`tools/textures/<NN-slot>/`)

**Status:** Accepted (2026-05-18)

**Context:** ADR-025 locks the **what** of the starter texture pack
(palette, sources, licences). This ADR locks the **where** and the
**shape** — the on-disk layout that the future
`barme-core::splat` registry (D3 / Sprint 8) will scan, and that the
splat tool inspector (D5 / Sprint 9) will display. Once D3 ships, the
contract becomes load-bearing; renaming slots or restructuring
directories becomes a refactor. Better to write it down now.

**Decision:**
- **Directory layout** rooted at `tools/textures/`:

```
tools/textures/
├── 00-grass-meadow/
│   ├── diffuse.{png,jpg}
│   ├── normal.png
│   └── meta.toml
├── 01-forest-floor-pine/
│   ├── diffuse.{png,jpg}
│   ├── normal.png
│   └── meta.toml
├── … (14 more) …
└── 15-alien-organic-creep/
    ├── diffuse.{png,jpg}
    ├── normal.png
    └── meta.toml
```

- **Slot directory name** is `NN-kebab-name`. `NN` is zero-padded 0..15;
  it survives the sort order of `std::fs::read_dir` cross-platform.
  `kebab-name` matches `meta.toml`'s `slot` index + `name` — D3's
  registry asserts the two agree at scan time and emits a warning on
  drift.
- **`meta.toml` schema** (TOML 1.0; serde-friendly):
  ```toml
  slot = 0                          # u8, must equal the prefix in the dirname
  name = "Grass meadow"             # human label (UI)
  biome = "Earth-Temperate"         # one of: Earth-Temperate, Arid,
                                    # Snow-Alpine, Alien-Industrial
  source = "https://ambientcg.com/view?id=Grass002"
  license = "CC0-1.0"               # SPDX identifier
  default_tex_scale = 0.02          # initial value for splats.tex_scale
  default_tex_mult = 1.0            # initial value for splats.tex_mult
  ```
- **File contracts**:
  - `diffuse.{png,jpg}` — sRGB colour. PNG when sourced from
    `_1K-PNG.zip` (the current default). JPG remains a permitted
    extension for future user-import (F23) of smaller diffuse-only
    assets. Either extension is acceptable; the registry probes both
    when loading.
  - `normal.png` — OpenGL tangent space (Y up), R=X, G=Y, B=Z. **PNG
    only.** JPG is rejected; the registry emits an error if it sees
    `normal.jpg`.
  - `meta.toml` — UTF-8 TOML, one slot per file. No nested tables.
- **Idempotency contract**: `meta.toml` is auto-generated by
  `scripts/fetch-textures.sh`; the file header says so. Hand-edits
  are overwritten on the next fetch. Per-project overrides (e.g.
  user dragged a tex_scale slider) live in `Project.splats[]`, not
  in `meta.toml`.
- **Why TOML, not JSON / RON?** TOML is the workspace's existing
  config format (`Cargo.toml`, `.barmeproj` already uses
  TOML-via-serde). Future D3 registry can deserialise with `toml`
  (a transitive dep of `cargo` itself; trivial to add).

**Alternatives:**
- **Flat directory + JSON manifest** (single
  `tools/textures/manifest.json` with all 16 slots inside).
  Rejected: pushes every per-slot rename into a single point of
  edit, complicates F23 user-import (each user-added slot would
  need a `manifest.json` patch).
- **Slot-by-name only** (`tools/textures/grass-meadow/`). Rejected:
  splat distribution textures are indexed by ordinal in mapinfo's
  `splatDetailNormalTex[1..4]` — the registry needs a stable u8 →
  slot mapping anyway. Putting the index in the directory name
  makes the mapping visible without opening files.
- **Numeric-only directories** (`tools/textures/00/`). Rejected:
  `ls tools/textures/` becomes opaque; the slot name is too useful
  to drop.
- **Folding ADR-027 into ADR-025**. Considered. Kept separate
  because ADR-025 is the "what's in the pack" decision (could
  change when we bump assets) and ADR-027 is the "shape on disk"
  contract (changing this is a breaking refactor for D3). The two
  evolve at different cadences; keeping them split is cheaper
  long-term.

**Consequence:**
- `scripts/fetch-textures.sh` writes the layout above for all 16
  slots. The script is the canonical writer of `meta.toml`.
- D3's `barme-core::splat::registry` (Sprint 8) scans
  `tools/textures/` and yields `Vec<SlotMeta>` ordered by `slot`.
  Drift checks: dirname prefix must equal `slot`; `slot` must be
  unique; `slot` must be in `0..=15` (for the starter pack; the
  registry tolerates higher values for F23 user-imports).
- D5's splat tool inspector (Sprint 9) reads `name` + `biome` +
  `default_tex_scale` from each slot's `meta.toml` to populate the
  4×4 palette grid.
- The `splatDetailNormalDiffuseAlpha` flag (per-project, set via
  the F9 form editor at C7) is NOT in `meta.toml` — it's a
  project-level toggle, not a slot-level one.
- Future user-import (F23 / Phase 6) reuses the same layout.
  Out-of-pack slots get index ≥ 16 and skip the dirname-prefix
  drift check.

**Sprint 17 update (ADR-041, D10, 2026-05-20):** ADR-027 covers
the stock-slot layout under `tools/textures/`. Sprint 17 adds a
second source location for `LayerSource::Imported` entries:
`<project>/textures/<uuid>.png` + a sibling `<uuid>.meta.toml`
with `name`, `source_filename`, `original_dims`, `imported_at_unix`.

The two locations are disjoint:
- **Stock slots** live under the shared `tools/textures/<NN-slug>/`
  tree, discovered once at app start, share thumbnails across all
  projects.
- **Imported textures** live under each `.barmeproj`'s parent
  directory + a `textures/` subdirectory. Paths are stored
  project-relative on `LayerSource::Imported { path }` so a
  moved project carries its textures with it.

`SlotResolver::imported_root` (default-method addition in Sprint 17)
returns the directory relative-imported paths resolve against;
`AppSlotResolver::with_project_root` threads the `.barmeproj`
parent through to the bake. Sprint 17's `App::migrate_imported_
layer_paths` re-homes any pre-Sprint-17 absolute / non-`textures/`
paths into the canonical sidecar at load time.

UUIDs come from `barme_core::alloc_layer_id` (the same UUID-shaped
hex `TextureLayer.id` allocator). PITFALL §17.4 — imports refuse to
run when the project isn't saved (no target dir to copy into).
PITFALL §17.5 — importing marks the project dirty so a
user-quit-without-save doesn't dangle the imported file.

## Template for new entries

## ADR-036 — Terrain fragment shader: splat-blended diffuse composite (Sprint 9 / D4)

**Status:** Accepted 2026-05-18; **superseded by ADR-043** 2026-05-21
(Sprint 25 / R1). The diffuse-only simplification described here
served its purpose for Sprint 9's preview MVP; Sprint 25 ports the
full `SMFFragProg.glsl` `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` branch
(base-normal sampling, per-fragment TBN, DNTS normal blending, per-
fragment specular). The `splatCofac` math itself carries forward
into ADR-043 unchanged; what changes is which textures the cofactor
weights — Sprint 25 swaps the slot DIFFUSE array (whose role retired
in Sprint 17's ADR-041 when the composite RT took over the diffuse
base) for the slot NORMAL array.

Originally superseded the draft research note at
`docs/research/splat-rendering/claude findings.md` (drafted as
"ADR-035" before the UI overhaul claimed that slot).

**Context.** The Stage 1 fragment shader at
`crates/barme-app/src/terrain.wgsl` previously rendered a height-only
biome gradient (ADR-017). Sprint 8 (D2 + D3) landed the DNTS bake
pipeline + `SplatDistribution` brushes; Sprint 9's D4 wires those into
the GPU preview so users see the painted slot diffuse instead of the
fallback gradient. The Recoil engine source is the authority, sampled
through `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
(SMF_DETAIL_NORMAL_TEXTURE_SPLATTING branch) — but the editor's
preview budget is bounded by the 8 ms brush-stroke NFR and there is
no baked SMT normal texture at edit time, so we adopt a **diffuse-
only** simplification of the engine composite.

**Decision — composite math.** Mirror the engine's `GetSplatDetailTexture
Normal` shape (SMFFragProg.glsl:174-198) per FINDINGS §7.3:

```
splatCofac = textureSample(splatDistr, uv_norm) * texMults * active_mask
detail_strength = min(1.0, dot(splatCofac, vec4(1.0)))
splat_rgb =
      textureSample(slot_diffuse, world.xz * tex_scales.r, 0).rgb * splatCofac.r
    + textureSample(slot_diffuse, world.xz * tex_scales.g, 1).rgb * splatCofac.g
    + textureSample(slot_diffuse, world.xz * tex_scales.b, 2).rgb * splatCofac.b
    + textureSample(slot_diffuse, world.xz * tex_scales.a, 3).rgb * splatCofac.a
base_rgb = mix(biome_gradient(world.y / max_h), splat_rgb, detail_strength)
lit = base_rgb * (ground_ambient + ground_diffuse * max(dot(n, sun), 0))
```

Key fidelity preserved:
- Per-channel UV stream `worldPos.xz * splatTexScales[i]` (SMFFragProg
  lines 175-176). NOT a divide.
- `splatCofac = dist * texMults` applied to the **whole vec4** (not
  per-RGB).
- Saturating weight sum `min(1.0, dot(splatCofac, vec4(1.0)))` mirrors
  `splatDetailStrength.x` (SMFFragProg:180).
- `splats.texScales` default `vec4(0.02)` and `splats.texMults`
  default `vec4(1.0)` per FINDINGS §1.6.
- `SMF_INTENSITY_MULT = 210/255` (FINDINGS §7.1 — with T) pre-applied
  CPU-side on `ground_ambient` so the WGSL stays clean.

**Decision — texture array.** The four slot diffuse images bind as one
`texture_2d_array` (`SLOT_LAYER_COUNT = 4`, `SLOT_DIFFUSE_DIM = 1024`,
`rgba8unorm`). Slot reassignment becomes
`queue.write_texture(layer = N)` — one frame's stall — rather than a
bind-group rebuild. Sized to match the ambientCG `_1K-PNG.zip`
starter pack (ADR-025); F23 user-imports resize-and-cache on the
upload side.

**Decision — splat distribution upload.** Mirror ADR-017 dirty-rect
sub-upload via `write_splat_rect(rect, full_data)`. Full-texture
writes at 4 MB per stamp blow the 8 ms NFR; D3's `SplatBrush::apply`
already returns a tight `DirtyRect` for each stamp, unioned across
symmetry replicas (ADR-019 pattern).

**Decision — base normal.** Computed per-vertex from a 4-tap finite
difference of neighbouring heightmap samples; interpolated to the
fragment as `world_normal`. The engine path samples a baked SMT
normal texture per-fragment (`normalsTex.ra` → X / Z, Y derived per
FINDINGS §7.5); the editor has no SMT bake at preview time, and the
heightmap-derived normal is sufficient for diffuse Lambert shading.

**Decision — uniforms layout.** New `SplatU` block (binding 2,
fragment-visible):
```wgsl
struct SplatU {
  tex_scales: vec4<f32>,
  tex_mults:  vec4<f32>,
  flags:      vec4<u32>,   // .x = active_slot_mask, .y = diffuse_in_alpha
  sun_dir:    vec4<f32>,
  ground_ambient: vec4<f32>,
  ground_diffuse: vec4<f32>,
};
```
CPU mirror at `render::SplatUniforms` with `#[repr(C)]` and
`bytemuck::Pod`. The active-slot mask gates per-layer sampling so a
fresh project (no slots bound) reads 0 from all four layers and falls
back to the height gradient.

**Alternatives considered.**
- **DNTS-normal blending in the preview** (full engine path). Rejected:
  requires a baked SMT normal map the editor doesn't have at preview
  time, and the per-fragment TBN math (FINDINGS §7.4 — `cross(normal,
  vec3(-1,0,0))`) doubles fragment cost. Promote to a follow-up ADR
  if a tested BAR-map screenshot vs the editor preview visibly
  disagrees on slope shading.
- **Static `T = +X / B = +Z / N = +Y` TBN basis** (per the draft research
  WGSL). Rejected: works on flat ground, visibly skews on slopes
  (FINDINGS §7.4 explicit correction). Moot since this ADR skips DNTS
  normals entirely.
- **Verbatim WGSL from `docs/research/splat-rendering/claude findings.md`**
  (drafted as "ADR-035"). Rejected: that draft has five load-bearing
  bugs catalogued in FINDINGS §7.1-§7.6 (`SMF_INTENSITY_MUL` typo,
  static tangent basis, generic RGB normal decode, `mix(16,
  specularExponent, alpha)` exponent, `(α-0.5)*2*w` diffuse offset
  sign). This ADR translates from the engine source directly and
  cites FINDINGS for each delta.

**Consequences.**
- Bind group grows from 2 entries (uniforms, heightmap) to 7
  (+ splat uniforms, distribution texture, distribution sampler,
  slot diffuse array, slot diffuse sampler). Comfortably within
  wgpu's 16-binding default.
- `terrain.wgsl` fragment stage gains the splat composite + Lambert
  lighting (replacing the height-only gradient).
- `render::TerrainCallback::new` signature gains `world_extent_x`,
  `world_extent_z`, and `SplatUniforms` parameters. Call site in
  `App::central` (`main.rs`) updates to pass them.
- New public APIs: `render::upload_splat_distribution`,
  `render::write_splat_rect`, `render::upload_diffuse_layer`,
  `render::update_splat_uniforms`. D5 wires the inspector to call
  each.

**Editor-preview deferrals (FINDINGS §7 caveats).** Each item lists the
trigger that should promote it from "deferred" to "implemented":
1. **No DNTS-normal blend.** Promote when a BAR-map screenshot vs the
   preview visibly disagrees on slope shading.
2. **No `splatDetailNormalDiffuseAlpha` (high-pass diffuse offset).**
   ADR-034 reserved; D5's GLOBAL section surfaces the toggle but the
   shader treats it as a no-op. Promote with the alpha-encoded
   high-pass bake.
3. **No shadows; `groundShadowDensity` collapsed to 1.0.** Promote when
   the editor gains a one-light shadow-map pass for unit placement
   preview (Sprint 12+).
4. **No specular.** A specular texture is not yet authored by the
   editor; the lint rule from FINDINGS §7.2 surfaces in the validation
   chip when a slot is bound and no specular is set. Promote when
   the F4-specular branch (Sprint 14+) lands.
5. **No `skyReflectModTex` cube modulation, no water absorption blend,
   no fog blend, no atmospheric scattering.** Out of scope per
   FINDINGS §7 explicit non-goals.

**Pitfall notes.**
- **DNTS-normal Y-flip.** The Sprint 8 closing devlog flagged that the
  DNTS shader composite (when we add it) must NOT add a second Y-flip
  on top of `BakeOptions::yflip_normal = false`; the bake already
  preserves OpenGL convention from ambientCG's `*_NormalGL.png`
  source. This ADR ships the diffuse-only path so the concern is
  parked until the DNTS-normal branch lands.
- **No specular gating regression.** Engine current behaviour
  (`SMFRenderState.cpp:114`, FINDINGS §7.2) gates DNTS on
  `splatDistrTex && splatDetailNormalTex[].size() > 0` only.
  `specularTex` is no longer in the gate; the editor's lint stays as
  a yellow warning per the reworded FINDINGS wording.

**Reference.** `docs/research/source-audit-2026-05-18/FINDINGS.md` §7 +
`RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl`
lines 146-150, 174-198, 276-278, 412-413, 4 (the `SMF_INTENSITY_MULT`
macro).

## ADR-037 — Offscreen render target + GPU markers + line pipeline (Sprint 13)

**Status:** Accepted (2026-05-19). Foundation of the renderer-parity
arc (Sprints 13 + 20-27, sketched at
`docs/research/renderer-bar-parity/ROADMAP.md`).

**Context.** Through Sprint 12 the editor renderer (`crates/barme-app/src/render.rs`)
created the terrain pipeline with `depth_stencil: None` and rendered
directly into the egui surface via `egui_wgpu::Callback::paint`. Every
3D-positioned UI element (start positions, metal spots, geo vents,
brush rings, mirror ghosts, symmetry axes) was CPU-projected via
`render::world_to_screen` and painted as flat `egui::Painter` shapes on
top of the terrain pass. Two visible defects followed (renderer audit,
`devlog/stage-1-sprint-prompts-audit/notes.md`):

1. **Translucent markers blended in iteration order, not depth order.**
   Orbit the camera 180° and a marker that should be behind still drew
   on top.
2. **Markers could not be occluded by terrain.** A start position
   behind a mountain showed through.

The 2026-05-18 user direction reversed SRS §2.1 #11 (3D preview ≠
in-game rendering): the editor must visually reproduce what Recoil
renders for terrain, atmosphere, water, shadows, features, grass, and
emission. Sprint 13 ships the foundation for that arc; the depth +
GPU-marker plumbing it lays down is a prerequisite for every
subsequent renderer-parity sprint.

**Decision.** Three coupled pieces:

1. **Offscreen render target** (`render::OffscreenTarget`):
   `Rgba8UnormSrgb` colour + `Depth32Float` depth, allocated lazily by
   `render::ensure_offscreen` on the first frame with a real central
   viewport rect. Sized to `min(rect_physical, 2048²)` per axis
   (PITFALLS §1 — iGPU memory budget at 4K × DPR=2 caps the offscreen
   RT at ~32 MB instead of 256 MB). Re-registered with
   `egui_wgpu::Renderer::register_native_texture` on every size change
   (old id freed first to avoid handle leaks).
2. **GPU marker pipeline** (`crates/barme-app/src/markers.wgsl` +
   `crates/barme-app/src/ui/markers.rs`): a `TriangleStrip` shader
   that draws billboarded screen-space markers (filled circle, outline
   ring, filled-with-stroke, filled triangle, outline triangle) from a
   pre-allocated 10 000-instance storage buffer. Depth-test against
   terrain (which writes depth) but no depth-write of its own —
   translucent ordering is owned by `MarkerBatch::sort_back_to_front`'s
   CPU sort each frame.
3. **GPU line pipeline** (`crates/barme-app/src/lines.wgsl`): a
   `LineList` shader for world-space dashed symmetry axes and geo-vent
   plumes. Depth-test only, premultiplied-alpha blend, shares the
   marker pipeline's uniform buffer (only consumes the `view_proj`
   prefix).

All three pipelines encode into a single `wgpu::RenderPass` inside
`TerrainCallback::prepare` (not `paint`) so the depth attachment is
provisioned by us — egui's own pass has no depth target. The
Callback's `paint` is a no-op; `central()` composites the offscreen
colour into the viewport via `ui.painter().image(offscreen_id, rect,
...)` immediately after adding the Callback. 2D residue (text labels,
viewport chrome, rulers, minimap) paints on top using the standard
egui painter.

`OrbitCamera::near_far` was added in the same sprint (Phase 3) to
auto-tune the depth-precision window from `distance` (near = 1 % of
distance floored at 50, far = 4 × distance with a 100× ratio floor).
`world_to_screen` was relaxed to drop the `|ndc.xy| > 1` rejection
(Phase 6) so label projection agrees with the GPU rasterizer on
screen-edge points; behind-camera (`clip.w <= 0`) still returns `None`.

**Alternatives considered.**

- **Sort markers in `egui::Painter` (CPU 2D).** Rejected — solves the
  blending defect but not terrain occlusion (would need a CPU-side
  depth read or world raycast per marker, both more expensive than a
  GPU depth-test) and leaves no GPU foundation for Sprints 20-27.
- **Fork `egui-wgpu` to provision a depth target on its own pass.**
  Rejected — ties us to a specific eframe version and fights the
  upstream model (egui's painter is intentionally depth-agnostic).
- **MSAA on the offscreen RT.** Deferred to a future polish item;
  needs `sample_count > 1` on every pipeline + a resolve attachment.
  Not blocking parity; can land any time once Phase 1 is stable.
- **HDR (`Rgba16Float` + tone-map).** Stage-2 ask; format change here
  would ripple through the entire renderer-parity arc.

**Consequences.**

- Terrain occludes markers correctly; translucent markers blend in
  correct camera-relative order during orbit.
- Memory: +16 MB / Mpixel offscreen colour + +16 MB / Mpixel depth =
  ~32 MB at the 2048² clamp. At 1080p (~1.3 MB / pixel = ~24 MB) the
  cap doesn't engage.
- Code: `crates/barme-app/src/render.rs` gains `OffscreenTarget`,
  `MarkerResources`, `LineResources`, `MarkerU`, `LineVertex`. New
  shaders `markers.wgsl` (130 LoC) + `lines.wgsl` (43 LoC). New
  module `crates/barme-app/src/ui/markers.rs` (~400 LoC) owning the
  CPU-side `Marker` / `MarkerBatch` / `MarkerInstanceGpu` /
  `project_to_screen`.
- `crates/barme-app/src/main.rs::central` restructured: PHASE A builds
  the marker + line batches by walking every visible marker source;
  PHASE B adds the Callback + image composite; PHASE C paints the 2D
  residue (text labels, viewport chrome) on top.
- `overlay.rs`: `paint_brush_ghosts`, `paint_primary_brush_ring`,
  `paint_symmetry_overlay`, and `paint_dashed_segment` retired in
  favour of `collect_brush_ghosts`, `collect_primary_brush_ring`, and
  `collect_symmetry_segments` (push into a `MarkerBatch` / line
  vertex `Vec` instead of into an `egui::Painter`).
- Multithreading: not used in Sprint 13's path. Marker counts are
  <10k and the stable `Vec::sort_by` runs sub-millisecond. Escape
  hatch documented in `ui/markers.rs` — swap to `rayon::par_sort_by`
  if the final-devlog perf table ever shows the sort exceeding 1 ms.

**STATUS UPDATE 2026-05-19 (hotfix):** live smoke testing surfaced
two issues that ship as two follow-on commits on `main` without a new
ADR. (1) PHASE A marker construction in `central()` hard-coded world
Y = 0; the 2-elmo `MARKER_Y_LIFT_ELMOS` epsilon was sized for h=0
z-fight, not arbitrary terrain elevations, so any non-flat map
(parabolic bowl with max_height ≈ 1236 reproduced it) buried metal /
geo / start-position markers under the surface — `App::terrain_y_at`
now lifts every PHASE A marker push site (and the geo-plume base) to
the heightmap-sampled surface, leaving `into_instances`'s lift to add
the small epsilon on top. (2) `collect_symmetry_segments` overflowed
the 5 000-vert line buffer at extreme zoom-in (10 000+ projected
dashes); the symmetry axis is now Liang–Barsky-clipped to the
visible rect (64 px margin) before dashing, with a per-axis dash cap
of 256 falling back to a solid segment past the threshold, and
`LINE_VERTEX_CAPACITY` bumped 5 000 → 8 000 as belt-and-suspenders.
Brush ring lift and per-vertex terrain Y on the symmetry axes remain
explicitly deferred (brush wants GPU ray-vs-heightmap picking; axes
are thin 1-px lines where z-fight is far less visible than missing
markers). Devlog: `devlog/stage-1-renderer-depth-rework-hotfix/`.

**Pitfalls (operational notes for the renderer-parity arc).**

- `Callback::prepare`'s encoder is shared with egui — do **not**
  `encoder.finish()` ourselves; return `Vec::new()`.
- Pipeline colour-target format must match the offscreen view's
  format (`Rgba8UnormSrgb`), **not** `render_state.target_format`.
  Mismatches surface as a pipeline-creation validation error.
- Depth texture must have `RENDER_ATTACHMENT` usage (`TEXTURE_BINDING`
  alone is rejected).
- `Depth32Float` works on every modern desktop adapter; fall back to
  `Depth32FloatStencil8` only if a future ARM/Linux target lacks it.
- Premultiplied alpha is mandatory across the marker + line
  pipelines (blend = `PREMULTIPLIED_ALPHA_BLENDING`, shader outputs
  premul colour, `egui::Color32` is internally premul).
- Markers are lifted by `MARKER_Y_LIFT_ELMOS = 2.0` in world space
  before the GPU upload (`MarkerBatch::into_instances`) so a marker
  at terrain h=0 doesn't z-fight the ground.
- `egui_wgpu::Renderer::register_native_texture` returns a fresh
  `TextureId` per call. Re-register **only** when the offscreen RT
  size changes; the old id is freed via `Renderer::free_texture`
  before allocating the new one to prevent handle leaks.

**Reference.** Renderer audit:
`devlog/stage-1-sprint-prompts-audit/notes.md`. Arc roadmap:
`docs/research/renderer-bar-parity/ROADMAP.md`. Sprint 13 prompt +
phase log: `docs/prompts/sprint-13-renderer-depth-rework.md`,
`devlog/stage-1-renderer-depth-rework/`.

## ADR-042 — Water + Lava as a map property (Sprint 14 / C9)

**Status:** Accepted (2026-05-19). Closes the "water emission gap"
flagged by the 2026-05-19 water-lava engine research
(`devlog/research-water-lava/logs/2026-05-19T10-59-15__water-lava-engine-research.md`).
Polished water rendering (foam / fresnel / caustics / lava emission)
deferred to the renderer-parity arc.

**Context.** Through Sprint 13 the editor had a complete
`barme_core::WaterBlock` (30+ fields, `bar_default_with_water()`
already populated) and an emitter (`barme-pipeline::mapinfo::water_block`)
that wrote every field with the correct Lua key. But `From<&Project>
for MapInfo` (`mapinfo_schema.rs:758`) always called plain
`bar_default()`, leaving `info.water = None` and shipping no `water = {
… }` sub-table even on maps with `min_height < 0`. BAR loaded those
maps with its engine-default blue ocean — visually wrong, but more
importantly, every mod gadget that read `mapinfo.water.X` would nil-
crash if it ever ran (cf. PITFALL §"three-gate"). The 30-field
`WaterBlock` was dead code; `bar_default_with_water` had no callers.

The research report also reframed the user's mental model:
`Ground.h::GetWaterPlaneLevel` is `consteval 0.0f`, so there's no
such thing as "paint where water is" or "place a lake at height 50"
— water is wherever `heightmap.y < 0`, period. A user clicking a
"water tool" expects to either pick a global palette (ocean / acid /
lava) AND/OR carve the heightmap below 0 with a brush. Both
affordances are missing in Stage 1 today.

**Decision.** Five coupled pieces:

1. **`WaterMode` preset enum** (`barme_core::water_presets`):
   `None | Ocean | Tropical | Acid | Lava | Magma | Custom`. Each
   non-None variant has a hand-written `WaterBlock` literal anchored
   to a real BAR map (Coastlines Dry, Gecko Isle Remake, Acidic
   Quarry, plus synthesised Lava / Magma). `#[serde(other)]` on
   `Custom` means a future preset (e.g. "Geyser") loaded by an
   older editor degrades to `Custom` instead of crashing —
   forward-compat by design.

2. **`Project.water_overrides: WaterBlock`** — sparse `Option<…>`
   overlay on top of the active preset. Per-field merge
   (`override.field.or(preset.field)`) so switching presets keeps
   the user's tweaks intact (Photoshop-style; `damage = 30` rides
   through Ocean → Acid → Magma). The emission path
   `From<&Project>` calls `preset_water_block(p.water_mode)` →
   `merge_overrides(&preset, &p.water_overrides)` → assigns the
   result to `info.water`. The `Custom` mode uses an empty preset
   so the user's overrides bleed through verbatim.

3. **Schema-versioned migration**: `Project.schema_v: u32` (current
   `Project::SCHEMA_V == 1`). On load, if `schema_v == 0`,
   `min_height < 0`, AND `water_mode` is default (None), the
   migration sets `water_mode = Ocean`. Bumps `schema_v` to 1 after.
   Runs exactly once per project — re-saved files carry `v = 1`
   and skip the rule. Critical for the "user explicitly chose None
   on an `min_height < 0` map" case: that choice survives reload.

4. **`Tool::Water` + flat plane MVP**: a 9th tool variant, keyboard
   `W`, `Icon::Water` (two stacked tilde waves). LMB-drag floods
   via `Brush::Lower`; RMB-drag raises. New `water.wgsl` renders a
   single alpha-blended quad at `y = 0` covering the map's XZ
   extent, tinted by `merged.surface_color * surface_alpha` (CPU
   pre-multiplied). Depth-test on / depth-write off; draws between
   terrain (writes depth) and markers (depth-test only), so
   cliffs occlude the plane but brush rings stay on top. Polished
   water (fresnel, foam, caustics, lava emission) is deferred to the
   renderer-parity arc — the MVP cut alone makes the feature
   self-explanatory.

5. **Mutual-exclusion auto-resolve** (PITFALL §6): the emission path
   forces `merged.plane_color = None` when `info.void_water == true`
   and emits a `warn!`. Setting both keys silently breaks `voidWater`
   per `MapInfo.cpp`. The inspector greys the plane-colour picker
   while `void_water` is on so the user sees the gating.

**Alternatives considered.**

- **Paint water zones onto a mask layer.** Rejected — the engine
  has no per-zone water (`consteval` water level), so any
  zone-paint UI would be a lie. The "carve heightmap below 0"
  approach matches what the engine actually consumes.
- **Full-fledged `AtmosphereField` + per-field atmosphere
  overrides** to power the Lava / Magma atmosphere offer.
  Rejected for Sprint 14 — ~200 LOC of mirror machinery for a
  single one-click toggle. Shipped as a coarser
  `Project.lava_atmosphere: bool` + hardcoded patch (red-orange
  fog, dim warm sun, dusty clouds) the emission path applies on
  top of `bar_default()`. Sprint 18's F9 form ships the granular
  surface.
- **Generic `mapinfo_overrides: HashMap<String, toml::Value>`** as
  the only override storage. Rejected for water — the typed
  `WaterBlock` lets the inspector form-bind sliders to specific
  fields without a string-keyed indirection. The free-form HashMap
  stays for F9's general-purpose form (Sprint 18) but doesn't try
  to subsume the typed water / atmosphere paths.
- **Drag finalisation on slider edits (one diff per gesture
  instead of per frame).** Acknowledged as a polish gap; per-frame
  diffs give fine-grained but busy undo. Deferred to a follow-up
  — the `dragging_*_from` snapshot pattern (used by metal / geo /
  feature drags) maps cleanly here when it lands.

**Consequences.**

- Builds visually-correct water blocks for every preset, fixing the
  silent omission that shipped with Sprints 12 / 13. BAR mod
  gadgets reading `mapinfo.water.X` get real values instead of nil
  crashes.
- The editor's 3D preview shows where BAR's water level sits as a
  translucent plane any time `water_mode != None`. Active tool gets
  full alpha; otherwise 0.5× cross-tool ghost.
- The Inspector exposes ~10 water-block fields directly (Preset
  chips + Behaviour + Appearance + Flood + Advanced placeholder) +
  the lava-atmosphere offer. The remaining ~20 fields are reachable
  via Sprint 18's F9 raw-fields form, which reuses the same
  `Project.water_overrides` shape.
- Three new validation-chip warnings catch silent
  misconfigurations (`DNTS + water LOS bug`, `min_height < 0 with
  no water preset`, `water preset set with no below-zero terrain`).
- Schema version stamps every `.barmeproj`; future migrations
  append to `Project::run_migrations` without re-firing v1.
- `App.min_height: f32` plumbed (closes a bug where
  `snapshot_project` hard-coded `0.0` and dropped any wizard-set
  value on first save).

**Pitfalls (operational notes for follow-up sprints).**

- Water-block fields are exclusively `Option<…>`; `None` means "use
  the preset value." The emission path's per-field `or` chain
  preserves the right semantic — never `unwrap_or_default()` a
  user-facing field, that's the wrong sentinel.
- `void_water` and `tidal_strength` live at MapInfo TOP LEVEL, not
  inside `water = {}`. The inspector co-locates them for UX but
  the schema field is `Project.{void_water, tidal_strength}`. Don't
  confuse `Project.water_overrides` (sparse `WaterBlock`) with the
  top-level shadows.
- The "Auto-set min_height" button sets `min_height =
  min(0, water_carve_depth)`, NOT `min(0, observed_min)`. The
  original formula in the C9 prompt assumed the heightmap encoded
  signed world Y directly; in practice the u16 lives in
  `[0, u16::MAX]` linearly mapped to `[min_height, max_height]`,
  and the observed-min computation collapses to `min_height`
  itself.
- Lava / Magma damage thresholds: `>= 1e3` blocks ground;
  `>= 1e4` blocks hovers. Lava sits at 1000 (ground-block, hovers
  allowed); Magma at 5000 (deep ground-block, hover-block ceiling
  untouched). Never silently land damage `>= 1e4` — hover gameplay
  is BAR-central.

**Reference.** Research report:
`devlog/research-water-lava/logs/2026-05-19T10-59-15__water-lava-engine-research.md`.
Sprint prompt: `docs/prompts/sprint-14-water-and-lava.md`. Devlogs:
`devlog/stage-1-water-{data-and-emission,preview-plane,tool-and-inspector}/`.

**STATUS UPDATE 2026-05-19 (post-C9 smoke).** First user run of
Tool::Water surfaced three issues; all fixed before this STATUS
line lands. The data path + emission flow described above was
correct, but the renderer + camera UX wasn't quite usable as
shipped.

1. **`Project.min_height` was inert in the terrain shader.**
   `sample_y` mapped raw `u16` to `[0, max_height]` regardless of
   `min_height`. Even after the C9 inspector's "Auto-set" button
   wrote `min_height = -100`, the heightmap rendered as if it
   still started at `Y = 0` — so BAR's water plane (also at
   `Y = 0`) sat flush with the floor and was invisible. Fixed by
   extending the terrain `Uniforms` with `params2: vec4<f32>`
   (`.x = min_height`, `.yzw` reserved) and updating `sample_y` to
   compute `y = min_h + t * (max_h - min_h)`. The fragment
   biome-ramp also rescaled so submerged terrain gets a distinct
   gradient colour instead of clamping at the lowest band. New
   PITFALL §28 captures this.

2. **Inspector Flood section rewritten for clarity.** Pre-fix the
   section had a `carve_depth` DragValue + a placeholder "Auto-set
   min_height" button — no direct way to set the sea-floor depth,
   no explanation of why a user would. Post-fix the section opens
   with the explainer "BAR's water plane is fixed at Y = 0. To
   make water visible, set min_height below zero…", then exposes
   a directly-editable **Sea-floor depth** DragValue
   (range `-2048..=0`). The carve-depth control + the "Set sea
   floor to carve depth" shortcut button stay.

3. **Arrow-key camera pan + recenter button.** Adjacent UX gap the
   smoke exposed: navigating an off-the-map view required either
   re-running the wizard or manually orbiting. Added arrow-key
   pan (delta-time-scaled velocity, Shift = 3× faster) and a
   Compass-icon recenter button in `top_bar_right_block` that
   calls `App::recenter_camera`. Rulers rewritten to be zoom-
   aware (`viewport_chrome::paint_rulers` now takes the camera
   and projects screen positions back to world XZ via
   `screen_to_world_y0`, then labels at 1-2-5 step sizes from
   the visible world range). The first arrow-key implementation
   had left/right inverted; PITFALL §27 documents the glam
   `look_at_lh` sign-flip convention that caused it.

Test counts after smoke follow-ups: barme-core 196 / barme-app
232 / barme-pipeline 114 (+1 net new since the rollup, for the
`pick_nice_step` and `interp_screen_pos` ruler-math pins). cargo
fmt / clippy / test all green.

## ADR-038 — Layered texture stack data model + CPU bake-to-diffuse (Sprint 15 / D8)

**Status:** Accepted (2026-05-19). Lands the data + bake half of
the layered-painter trio. The paint viewport and GPU composite are
ADR-039 / ADR-040 (Sprint 16); the Layers panel + DNTS hybrid
emission is ADR-041 (Sprint 17).

**Context.** Through Sprint 14 the `.sd7` diffuse was the synthetic
biome ramp baked by `crates/barme-app/src/launcher.rs::synth_biome_bmp`
(introduced commit `f1ab09b`). On 2026-05-19 the user reported that
the exported map textures are "incredibly ugly." The root cause is
twofold: (1) the synth ramp is height-keyed, so a flat map ships
with a flat single-tone diffuse; (2) BAR's actual diffuse channel
is `512 × SMU` per side (8192² for 16 SMU) and meant to be hand-
composited from multiple texture layers — what the 4-channel splat
distribution wires up in Sprint 12 is the **DNTS detail** path
(BAR's per-fragment normal/specular detail overlay), NOT the
diffuse itself. The two channels live ON TOP OF each other in the
engine's frag shader (`SMFFragProg.glsl::GetFragmentDiffuse` +
`GetSplatDetailNormal` per FINDINGS §7.3).

The painter rebuild is split across three sprints to keep blast
radius bounded and let the data shape stabilise before any UI
lands on top:

- **Sprint 15 / D8 (this ADR):** data model + CPU bake + .sd7
  hookup. **No UI.**
- **Sprint 16 / D9:** tiled-COW masks + paint viewport + GPU
  composite preview shader.
- **Sprint 17 / D10:** Photoshop-style Layers panel + custom
  texture import + DNTS hybrid emission (bottom ≤ 4 DNTS-bound
  layers drive the splat distribution + DDS bake too, retiring
  `inspector_splat` + `Tool::SplatPaint`).

**Decision.** New `barme-core::layers` module:

- **`LayerStack`** — `Vec<TextureLayer>` ordered bottom-first (idx
  0 = bottom). The compositor iterates naturally; the Sprint 17
  Layers panel renders the view in reverse to match Photoshop
  convention.

- **`TextureLayer`** carries a stable `id: String` (16-hex-char
  time + counter mix — UUID-shape but dependency-free), a
  user-visible `name`, a `LayerSource` (`Slot { id }` or
  `Imported { path }`), an affine `LayerTransform` (offset +
  scale + rotation + mirror_x/y), a `LayerColor` (RGB tint +
  brightness), a `BlendMode::Normal` (alpha-over only for v1 —
  the enum is reserved so a future `Multiply` doesn't silently
  degrade in the compositor), `visible` / `locked` / `opacity`,
  an optional `dnts_channel: Option<SplatChannel>` (Sprint 17
  uses this to identify the bottom ≤ 4 layers that drive the
  splat distribution + DNTS DDS bake), and a per-layer
  `LayerMask`.

- **`LayerMask`** is `width × height × Vec<u8>`, sized to the
  diffuse (`512 × SMU` per side). Sprint 15 ships the flat
  allocation with a `// TODO(tiled-cow)` flag at the alloc site
  — Sprint 16 (D9) swaps the storage for a tiled-COW structure
  adapted from ADR-018's heightmap pattern; the public API
  (`filled` / `sample` / `write_rect`) stays. TOML serde uses
  base64 (`mask_bytes_b64` mod) for now; D9's migration ships
  sidecar PNGs at `<project>/masks/<layer_id>.png` once tiled-
  COW is in place.

- **`SlotResolver` trait** — `fn diffuse_path(&self, slot_id: u8)
  -> Option<PathBuf>`. The trait requires `Sync` so the bake can
  decode per-layer diffuses in parallel via rayon. `barme-app`
  ships `AppSlotResolver` wrapping its `slot_registry:
  Vec<SlotMeta>`; tests use the `ClosureSlotResolver` adapter.

**Compositor (`LayerStack::bake_diffuse`).**

1. Filter visible-and-non-zero-opacity layers, then **rayon
   `par_iter`** to decode each layer's source PNG concurrently —
   `image::open` blocks the calling thread, so an 8-layer stack
   would otherwise serialise 8 PNG decodes.

2. Allocate an RGB8 output buffer at `size.texture_dims()`. The
   debug-assert `width / height ≥ 1024 AND multiple of 1024`
   pins the PyMapConv contract (`mapx = width / 8` requires
   multiples of 8 × 128 = 1024 — `512 × SMU` for `SMU ≥ 2`
   trivially satisfies).

3. **rayon `par_chunks_mut`** over rows — every output row is an
   independent destination. The per-pixel hot loop walks layers
   back-to-front and accumulates an alpha-over composite
   (`dst = src*α + dst*(1-α)`), flattens against a 0.18 sRGB
   mid-grey background to keep under-painted pixels readable,
   then rounds to u8.

4. Per-layer sampling is **wallpaper-tiled** (modulo) bilinear
   — edge-clamp would produce a "stretched smear at the seams"
   look the user explicitly rejected. The affine transform
   applies `mirror` BEFORE `rotation` (pinned by
   `bake_mirror_then_rotate_matches_reference`); reversing the
   order rotates the post-mirror axis the wrong way for any
   non-axis-aligned angle.

**Perf budget.** Target: ≤ 1.5 s release for 16-SMU × 8 layers.
Smoke shows 4-SMU × 2 layers at ~72 ms; linear-scaling gives
~4.5 s at 16-SMU × 8 — over budget. The rayon row parallelism
gives ~Nx speedup for N cores (8x on a typical dev box → ~570 ms
projected). The per-layer rayon decode helps at high layer
counts. If Sprint 17's bake still misses budget, the next lever
is to cache decoded sources on `LayerStack` rather than
re-decoding per bake.

**`Project.layers` + migration.** `Project` grows `pub layers:
LayerStack` with `#[serde(default)]` so pre-Sprint-15 .barmeproj
files load with an empty stack. `Project::after_load_migrate(&dyn
SlotResolver)` — idempotent, gated on `layers.is_empty()` — seeds
one layer per bound DNTS channel via
`LayerStack::migrate_from_splat_config`. The pre-D8
`splat_config` field stays as the source of truth for the
runtime DNTS path until Sprint 17; both live side-by-side
through Sprint 16.

**`ProjectDiff` variants.** Four new — `AddLayer { index, layer:
Box<TextureLayer> }`, `RemoveLayer { index, layer }`,
`ReorderLayer { from, to }`, `SetLayerProperty { layer_id, from,
to: LayerPropertyValue }`. `LayerPropertyValue` is a typed union
covering name / transform / color / blend / visible / locked /
opacity / dnts_channel / source. Mask-pixel edits are **NOT** a
ProjectDiff in Sprint 15 — no brushes touch the mask yet;
Sprint 16 (D9) lands a separate per-stroke COW path adapted from
ADR-033's heightmap-undo. The bytes accounting in
`ProjectDiff::bytes()` folds mask + string capacities so the
100 MB undo-cap eviction stays honest (a 16-SMU mask is 64 MB
and would otherwise sandbag the cap silently).

**Export hookup.** `barme-app::launcher::build_and_install` grows
a `slot_resolver: &dyn SlotResolver` parameter and a three-way
texture branch: (a) caller-supplied BMP → use as-is; (b)
non-empty `Project.layers` → bake via
`LayerStack::bake_diffuse`; (c) empty stack → fall back to the
Sprint 1 `synth_biome_bmp`. The fallback covers in-process
callers that build a bare `Project` (`barme-pipeline::examples::
build_smoke`, the integration tests). Splat distribution + DNTS
DDS emission continues to come from Sprint 12 / D6's
`splat_pipeline::stage_splat_assets` unchanged — Sprint 17
introduces the hybrid path.

**Caveat — DXT1 chroma drift.** PyMapConv DXT1-compresses the
output BMP per BAR's `compressionType=1, tileSize=32` mandate
(SRS §1.2 / PITFALL §4). Gradients and saturated chromas land
softer in the .sd7 than in the bake preview. Sprint 19's lint
pass will surface this if it gets bad; Sprint 15 just notes
it. The editor's preview composites against the same source
PNGs so the WYSIWYG gap is bounded to DXT1's quantisation —
within the renderer-parity arc's ΔE < 5.0 target.

**Alternatives considered.**

- *Keep `synth_biome_bmp` and improve the ramp.* Rejected: the
  biome ramp is height-keyed by definition; flat maps stay
  single-tone no matter how nice the ramp curves get. The
  user's "ugly" complaint is fundamentally about composition,
  not colour selection.

- *Skip the layered model; have the user paint directly into a
  full-resolution diffuse RGBA.* Rejected: a 16-SMU diffuse is
  192 MB raw; undoing strokes against the same buffer would
  push undo past the 100 MB cap on every long stroke. A layered
  model lets the masks stay 1 byte per pixel (¼ the raw cost)
  and per-stroke undo gates onto a single layer's mask via
  ADR-018's COW pattern in Sprint 16.

- *Bake on the GPU.* Deferred to Sprint 16 (D9 / ADR-039). The
  GPU pipeline gives a live preview shader; the CPU bake is
  authoritative for the `.sd7` because PyMapConv's input is a
  CPU-side BMP anyway. Two pipelines is a maintenance burden
  but the CPU side is needed regardless and lands first.

- *Use `LayerSource::Imported` only (no slot abstraction).*
  Rejected: the slot registry is the ADR-027 contract for
  cross-project diffuse reuse; making every base layer an
  imported path would force every fresh project to ship its
  own copy of `00-grass-meadow/diffuse.png` (~1 MB).

**Consequence.**

- `Project.layers` is the new authoritative source for the
  exported diffuse. `synth_biome_bmp` survives as a fallback
  for the empty-stack path; it is NOT a hot path post-Sprint
  15.
- `splat_config` stays as the source of truth for runtime DNTS
  through Sprint 16. Sprint 17 (ADR-041) retires it.
- The renderer's editor preview is unchanged in Sprint 15 — the
  3D viewport still composites via the Sprint 9 WGSL splat
  shader from the slot-bound diffuses. Sprint 16 lands the
  layered preview shader.
- The 100 MB undo cap honestly accounts for mask bytes via the
  new `ProjectDiff` variants. A 16-SMU mask under
  `AddLayer` / `RemoveLayer` is 64 MB and will dominate
  eviction priority — the largest-first policy (ADR-033) keeps
  smaller diffs alive.
- 4-SMU × 2 layers smoke baseline: 72 ms (recorded in
  `devlog/stage-1-layers-data-model/`). 16-SMU × 8 layers
  perf will be measured in Sprint 16's first commit cycle once
  the paint viewport can produce realistic stacks.

## ADR-039 — GPU layered composite pipeline + tiled-COW mask storage (Sprint 16 / D9)

**Status:** Accepted (2026-05-19). Lands the GPU live preview half
of the layered painter trio. The CPU bake (ADR-038) stays
authoritative for the `.sd7` export; this ADR adds the GPU
preview the paint viewport (ADR-040) and the terrain shader sample
on every frame. The data path that drives the GPU side — the
tiled-COW mask storage — ships under this same ADR because the
GPU upload contract (`dirty_tiles_since` + per-tile sub-uploads)
is defined relative to the storage shape.

**Context.** Sprint 15 / ADR-038 shipped [`LayerMask`] backed by a
flat `Vec<u8>`. That carries 64 MB per layer on a 16-SMU map
regardless of paint coverage — a 16-layer cap multiplied at 1 GB
before the user touched a brush, breaking the SRS NFR-Memory ≤ 4
GB resident budget at 16 SMU. The GPU side was also unwired —
the bake produced a BMP for export but the editor preview kept
sampling the Sprint-9 biome gradient + splat composite, so painting
into a layer didn't show up live.

**Decision.**

- **Tiled COW masks (`barme_core::layers::mask`).** Storage is a
  grid of 256² `Tile` cells, each either `Tile::Uniform(byte)` (16
  bytes resident) or `Tile::Pixels(Box<[u8; 65536]>)` (64 KB
  resident, allocated lazily on the first write that touches a
  uniform tile). `LayerMask::filled` returns an all-`Uniform` grid
  at ~16 KB regardless of map size; a typical brush stroke touches
  ~5–20 tiles for ~320 KB – 1.3 MB allocation. Memory scales with
  paint coverage, not map size — pinned by
  `tests::filled_layer_costs_under_one_kb_per_smu_axis`.

  The Sprint-15 wire format (`{ width, height, bytes: <base64> }`)
  loads cleanly via a custom `Deserialize` impl that scans each
  256² tile for runs of identical bytes and collapses to `Uniform`
  on import (`legacy_flat_bytes_round_trip_compresses_uniform_
  tiles`). Sprint-16+ serialises with the tile-discriminated wire
  shape (`tiles: Vec<String>` with `"u:<byte>"` / `"p:<b64>"`
  encoding); ~5 bytes per uniform tile, ~88 KB per concrete tile
  (base64 overhead included).

  `LayerMask::dirty_tiles_since(version)` + `version()` form the
  GPU upload cursor: callers capture the latest version after each
  upload pass, and the next call returns the tile coords with
  more-recent writes. A single brush stamp typically marks 1–4
  tiles dirty.

- **GPU composite pipeline (`crates/barme-app/src/composite.wgsl`).**
  Bind group: `CompositeU` uniform + 16-layer slot diffuse array
  + slot sampler (`Repeat` for wallpaper-tile) + 16-layer mask
  array (`r8unorm`, dims = composite RT) + mask sampler
  (`ClampToEdge` so mask edges don't tile). Fragment stage walks
  layers bottom-to-front, alpha-overing into an `acc_rgb`/`acc_a`
  pair, flattening against the same `0.18` mid-grey the CPU bake
  uses so the preview tone matches the export.

  16-layer cap: above 16, only the bottom 16 contribute to the
  preview; the CPU bake handles the full stack for `.sd7` export.
  The `dims` uniform packs `[rt_w, rt_h, world_extent_x_elmos,
  world_extent_z_elmos]` so layer offsets (in elmos) stay
  correct even when the RT clamps below `texture_dims` for >8-SMU
  maps.

- **Composite RT (`render::CompositeResources`).** `Rgba8Unorm`
  (NOT sRGB — matches the slot diffuse format so blending stays
  byte-true to the CPU path). Clamped at `COMPOSITE_RT_CLAMP =
  4096²`; maps >8 SMU clamp here and the terrain shader's bilinear
  sampler upscales when binding the RT as its diffuse base. The
  CPU bake stays full `texture_dims` for `.sd7` export.

- **Slot diffuse array.** 16-layer `texture_2d_array<rgba8unorm>`
  at 1024² per layer (`SLOT_COMPOSITE_DIM`). Pre-loaded once on
  app start via `App::reupload_layer_stack_diffuses`; per-layer
  rebind targets a single array layer via
  `render::upload_composite_slot_diffuse`. Layer N of the array
  matches layer N of `LayerStack::layers` — reorder / add /
  delete re-upload the affected layers to keep indices aligned.

- **Mask array.** 16-layer `texture_2d_array<r8unorm>` sized to the
  composite RT. Per-tile sub-uploads via
  `render::write_composite_layer_mask_tiles(layer_idx, &mask,
  &[TileCoord])` — reads each tile via `LayerMask::read_tile` into
  a stack buffer, calls `queue.write_texture` with
  `origin = (tx * TILE_DIM, ty * TILE_DIM, layer_idx)`. A
  `debug_assert!` pins the tile-grid-bounds invariant. **Full mask
  writes are explicitly avoided** — 4096² × 16 layers = 256 MB per
  frame and would blow the 8 ms NFR-Performance budget.

- **Terrain shader patch.** New `params2.y` flag plus binding 7 /
  8 (composite RT view + sampler). When the project has a non-empty
  layer stack, `params2.y = 1.0` switches the terrain shader's
  diffuse base from the Sprint-9 biome ramp + splat composite to a
  direct sample of the composite RT. The Sprint-9 DNTS detail-
  normal overlay stays unchanged — Sprint 17 / ADR-041 will move
  DNTS emission onto the layer model.

- **Per-frame dispatch.** `App::central` calls
  `render::ensure_composite_rt` + `App::sync_composite_mask_tiles`
  before building the `TerrainCallback`; the callback's
  `prepare()` encodes the composite pass into the RT before the
  terrain pass so the terrain shader's sample lands on this
  frame's bake. The pass is unconditional when a layer stack
  exists — Sprint 16 doesn't scissor-mask to dirty rects because
  the full-screen pass at 4096² is < 1 ms on iGPU and the
  simpler shape is easier to debug.

**Alternatives considered.**

- *Per-stroke full-mask upload.* 256 MB per frame at 4096² × 16
  layers. Blows the 8 ms NFR. Rejected.
- *GPU composite into the heightmap's existing offscreen RT.*
  Reuses the egui texture binding but couples the layered diffuse
  to the terrain's depth attachment, which doesn't apply to a 2D
  preview. Rejected.
- *Run the composite as a separate `egui_wgpu::Callback`.* Cleaner
  separation but doubles the encoder churn. Folding into
  `TerrainCallback::prepare` keeps both passes in one encoder.
- *Pack mirror+rotate into a 2×3 affine matrix.* The 4-scalar
  packing (`[mirror_x_sign, mirror_y_sign, cos, sin]`) is smaller
  (4 floats vs 6) and the WGSL math is one fewer multiplication
  per pixel.

**Consequence.**

- `barme-core` gains a `layers::mask` module with `TileGrid`,
  `Tile::Uniform`/`Pixels`, `TileCoord`, `MaskStamp`, and the
  `flood_fill` helper for `mask-fill`. `LayerStack::apply_brush`
  dispatches one stamp into the named layer's mask.
- `barme-core::layers::brushes` ships the four mask brushes
  (`mask-reveal` / `mask-hide` / `mask-smooth` / `mask-fill`)
  under the new `MaskBrush` trait — object-safe `Send + Sync +
  'static`, mirrors the `SplatBrush` shape from ADR-018 / Sprint
  9.
- `crates/barme-app/src/composite.wgsl` — the new pipeline.
- `crates/barme-app/src/render.rs` gains `CompositeResources`,
  `CompositeLayerU`, `CompositeU`, `ensure_composite_rt`,
  `write_composite_layer_mask_tiles`,
  `upload_composite_slot_diffuse`, `update_composite_uniforms`.
  The terrain bind-group layout grows entries 7 + 8 for the
  composite RT view + sampler; `make_bind_group` takes the
  composite view + sampler as new args.
- `barme-app::main` plumbs the per-frame composite-mask sync from
  `central()` and re-uploads slot diffuses on project open / new /
  wizard / structural layer edits.
- Caveat: mask-pixel undo is NOT in scope. The COW machinery is
  the foundation a future sprint (Sprint 19+) can hang per-stroke
  undo off, mirroring ADR-033's heightmap path.
- Caveat: the composite RT preview is approximate on >8-SMU maps
  (4096² clamp; bilinear upscale to the terrain shader's per-
  fragment sample). The CPU bake remains authoritative for `.sd7`
  export.

## ADR-040 — Top-down 2D paint viewport + brought-forward Layers panel (Sprint 16 / D9)

**Status:** Accepted (2026-05-19). Lands the user-facing
painter for the layered diffuse: the 2D paint viewport that
samples the composite RT (ADR-039) + the Layers panel UI that
manages the stack. Scoped originally as a "minimal active-layer
strip" pending Sprint 17 / ADR-041; expanded mid-sprint per user
direction to ship a full add / rename / delete / reorder / opacity
/ import experience so the painting workflow is self-sufficient
without Sprint 17's hybrid emission.

**Context.** Sprint 16's prompt scoped the inspector to a vertical
"active layer" chip strip on the right, with Sprint 17 / ADR-041
owning the Photoshop-style Layers panel. Mid-Sprint review
established that without the ability to add / reorder / rename /
delete / set per-layer opacity / import a texture, the painting
workflow couldn't actually be tested — every stroke would target
the same single default biome layer at mask=255, where reveal is
a no-op and hide only uncovers the mid-grey background. The user
explicitly requested the full panel before signing off on Sprint
16. ADR-041 still owns custom-texture sidecar storage (`<project>/
textures/<uuid>.png`) + DNTS hybrid emission + retirement of
`inspector_splat`; this ADR ships the UX side only.

**Decision.**

- **`Tool::PaintLayer` variant** — keyboard `L`, `Icon::Brush`,
  label "Paint layer". Slots between `Water` and `Procgen` in
  `Tool::ALL`. When active, the central viewport swaps the 3D
  `TerrainCallback` for the 2D `ui::paint_view::paint_view` call.

- **Top-down 2D paint viewport** (`crates/barme-app/src/ui/paint_
  view.rs`). Ortho projection of the composite RT into the central
  rect at 1:1 aspect, **letterboxed bands** when the viewport's
  aspect differs from the map's (PITFALL §8 — explicit no-stretch).
  Pan: middle-mouse-button drag in world-elmo space. Zoom: scroll
  wheel pivoted on the cursor, range 0.25× – 16× of the auto-fit
  factor. Double-click resets pan + zoom to auto-fit. Brush ring
  overlay at the cursor (accent-coloured stroke + inner pip);
  mask-only preview toggle chip (top-right, Sprint 17 finishes
  the actual grayscale render); status strip (bottom) with layer
  name + cursor elmo coord + mask byte at the cursor.

- **Pointer dispatch** — `central_paint_layer` resolves the cursor
  → world elmos via the paint_view's pan/zoom math, calls
  `apply_mask_brush_at_elmos` on `drag_started_by(LMB)` and
  `apply_mask_brush_along_drag` on each subsequent `dragged_by`.
  Drag interpolation: when delta > `spacing × radius`, intermediate
  stamps are emitted so fast 500-px drags don't leave gaps (PITFALL
  §3). Off-map cursor clips silently.

- **Layers panel** (`App::inspector_paint_layer`). Top-down list
  (Photoshop convention; vec is bottom-first). Per layer:
  visibility toggle (👁/—), inline-edit name (TextEdit; commits
  on focus loss / Enter), source label (`Slot 02` or `imp: foo.png`),
  up / down move arrows, delete `×` button. Below the name row:
  opacity slider (0..=1) + Import button. Clicking the card sets
  the layer active.

  Pre-iteration `RefCell<Vec<LayerAction>>` collects intents
  during the layer walk so structural mutations (delete /
  reorder / import) don't fight egui's per-frame mut-borrows or
  shift the index mid-loop.

- **Demo seed.** `App::seed_demo_accent_layer` runs in `App::new`
  + `new_project` + (transitively via `apply_wizard`); adds a
  second layer at slot 1 (mask=0) on top of the default biome's
  base so paint reveal/hide immediately produce visible results
  on the first stroke. Idempotent — bails when the stack already
  has > 1 layer or when the registry doesn't carry a second slot.

- **`paint_active_layer_id` persistence.** Lives on App; survives
  tool switches (re-entering `Tool::PaintLayer` resumes on the
  same layer). Cleared on `new_project` / `open_from` (a loaded
  project may have a different stack).

- **Brush dispatch through `LayerStack::apply_brush`** which
  honours the per-layer `locked` flag (Sprint 17 will surface a
  lock toggle in the panel; the data field is in place since
  Sprint 15 / ADR-038).

**Scope boundary with ADR-041 (Sprint 17).**

- This ADR ships: add / rename / delete / reorder / opacity /
  visibility / texture import via picked-path. The texture import
  uses the picked path directly — the file isn't copied into a
  project-local sidecar; ADR-041 still owns that migration.
- ADR-041 will further add: drag-to-reorder (vs the up/down
  arrows here), per-layer thumbnail, lock toggle, DNTS-channel
  binding chip, blend-mode selector, per-layer transform editor.
- ADR-041 also owns retirement of `inspector_splat` /
  `Tool::SplatPaint` and the DNTS hybrid emission (bottom ≤ 4
  DNTS-bound layers populate the splat distribution + DDS bake).
- Mask-only preview chip toggles state but the per-frame mask
  grayscale render is Sprint 17. Sprint 17's path: register the
  composite mask array's per-layer view as an egui native
  texture and overlay it via `painter.image`.
- Mask-only preview chip toggles state but doesn't render the
  grayscale overlay yet — Sprint 17.

**Alternatives considered.**

- *Strict spec — ship only the minimal chip strip and defer the
  panel to Sprint 17.* User overruled. The minimal strip can't
  demonstrate the painting workflow on a default project.
- *Drag-to-reorder via `egui::dnd_*`.* Cleaner UX but the egui
  drag-and-drop API has subtle quirks around hit-testing during
  drag-over. The up/down arrow pair ships now; Sprint 17 upgrades.
- *Project-local texture sidecar.* ADR-041 spec — would mean
  copying the picked file into `<project>/textures/<uuid>.png` and
  updating the LayerSource to point there. Sprint 16 uses the
  picked path directly so unsaved projects can import too.

**Consequence.**

- `crates/barme-app/src/ui/paint_view.rs` (new) — ortho viewport.
- `App::central_paint_layer` + `apply_mask_brush_at_elmos` +
  `apply_mask_brush_along_drag` — pointer dispatch + drag
  interpolation.
- `App::inspector_paint_layer` — Layers panel.
- `App::add_layer_at_top` / `delete_layer` / `reorder_layer` /
  `import_layer_texture` / `seed_demo_accent_layer` — layer CRUD.
- `Tool::PaintLayer` variant + keyboard `L` + `Icon::Brush`.
- `Tool::ALL` widens to 10 variants; the pinning test bumps to 10.
- App state: `paint_active_layer_id`, `paint_view_state`,
  `paint_brush_state`, `mask_brushes`, `paint_last_drag_pos`.
- Default brush radius bumped 64 → 192 elmos for visibility on
  the first stamp.

## ADR-041 — Layers panel UI + DNTS hybrid emission + legacy splat retirement (Sprint 17 / D10)

**Status:** Accepted.

Closes the F4 row of the SRS feature matrix end-to-end. Extends
(does not supersede) ADR-038 (data model + CPU bake) / ADR-039
(GPU composite + tiled-COW masks) / ADR-040 (paint viewport +
brought-forward Layers panel skeleton). Retires `Tool::SplatPaint`
+ `inspector_splat` + the per-channel `splat_config` /
`splat_distribution` model at the project boundary.

**Context.** Sprint 16 (ADR-039 + ADR-040) shipped the
layered-painter foundation + a minimal panel inline in
`App::inspector_paint_layer`. The user feedback at end-of-Sprint-16
was clear: the painter is testable but the UX gaps make stock
textures hard to use ("Add layer" picks whatever next-unused slot)
and the legacy 4-channel splat inspector still ships alongside the
new panel — two ways to do everything is a worse onboarding story
than one. Sprint 17 finishes the feature.

**Decisions:**

- **Lift the Layers panel out of `main.rs`.** New module
  `crates/barme-app/src/ui/layers_panel.rs`. `main.rs` was already
  > 11 000 LoC; the panel's complexity (drag-to-reorder + thumbnail
  cache + DNTS chip + transform/color/blend properties + footer
  chips + slot picker popups) doubled the inline code. The module
  exposes `pub fn render(app: &mut App, ui: &mut egui::Ui)`; the
  `Tool::PaintLayer` Inspector dispatch reduces to a 3-line
  forwarder + the session-only BRUSH section underneath.

- **Per-layer DNTS scale + mult fields, not a per-channel global.**
  `TextureLayer.dnts_tex_scale: f32` + `dnts_tex_mult: f32` (default
  `0.02` / `1.0`, per FINDINGS §1.6). The retired
  `SplatConfig.tex_scales` / `tex_mults` migrate into the per-layer
  fields at `LayerStack::migrate_from_splat_config` time
  (PITFALL §17.8). The runtime DNTS shader reads via
  `LayerStack::dnts_layers()` instead of `SplatConfig.channels`.

- **`dnts_diffuse_in_alpha` becomes a per-project setting**, not
  per-channel. The Layers-panel footer toggle drives
  `Project.dnts_diffuse_in_alpha: bool`. Migration carries the
  legacy `SplatConfig.diffuse_in_alpha` flag forward once.

- **Stock-texture picker is the primary Add-layer path.** Sprint 16's
  "Add layer = next unused slot" was a fallback; Sprint 17's primary
  click on "Add layer" opens a popup with a 3-column thumbnail grid
  of every stock slot. "Import texture from disk…" is the secondary
  affordance in the same popup. The active-layer Source section's
  "Change slot…" button opens the same picker. Extracted as
  `widgets::slot_picker_grid` so the Add flow + the Change flow
  share one widget. (User-driven addition mid-sprint.)

- **DNTS hybrid emission drives runtime detail from layer masks.**
  Bottom ≤4 DNTS-bound layers materialise into the splat
  distribution PNG via a box-filter downsample (PITFALL §17.2:
  NOT nearest-neighbour) to 1024². Each bound layer's slot bakes a
  per-channel DDS via the existing `bake_dnts`. Imported-source
  DNTS layers emit a `LintWarning::ImportedLayerDnts` and skip the
  DDS bake (no stock normal map). New entry point
  `barme_pipeline::stage_splat_assets_from_layers` parallel to
  Sprint 12's `stage_splat_assets`; `build_sd7` dispatches on
  `project.layers.layers.is_empty()`.

  The `R + G + B + A ≤ 255` invariant holds by construction (one
  channel per layer; channels independent in the materialisation).
  Pinned by `mask_to_splat_distr_invariant_rgba_under_255`.

- **Per-stroke mask undo adapts ADR-033 to tiled-COW masks.**
  Sprint 16 left mask edits non-undoable; Sprint 17 lands a
  tile-granular snapshot/diff. `LayerMask::clone_tile(coord)` +
  `restore_tile(coord, tile)` + `tile_coords_overlapping_rect(rect)`
  expose the storage API. `History::snapshot_mask_tile` (called
  per-tile BEFORE the brush writes) + `end_mask_stroke` (commits a
  `HistoryEntry::Mask` from before / after pairs, filtering no-op
  tiles). Bytes accounting sums `Tile::resident_bytes` per pair.

- **Project-local texture sidecar.** Imports copy into
  `<project>/textures/<uuid>.png` + write a
  `<uuid>.meta.toml` (name, source_filename, original_dims,
  imported_at_unix). `LayerSource::Imported { path }` stores a
  project-relative path so a moved project keeps its textures.
  `SlotResolver::imported_root` (default-method addition) lets
  the bake resolve relative paths; `AppSlotResolver::with_project_root`
  threads the `.barmeproj` parent through. PITFALL §17.4: import
  refuses to run when the project isn't yet saved.

- **Drag-to-reorder gates the 64 MB diffuse re-upload to drop only.**
  egui 0.33's `dnd_drag_source` / `dnd_drop_zone` per row. In-flight
  drags mutate a session-only `App::paint_drag_preview_order` so the
  panel renders the reordered position without committing; the drop
  fires exactly one `ProjectDiff::ReorderLayer` (via
  `App::reorder_layer`, which does the cache clear + diffuse
  re-upload). PITFALL §17.12.

- **`Project.splat_config` becomes `#[serde(skip_serializing)]`** at
  Commit 6. New saves drop the legacy block; old loads still hydrate
  via the wire-side `#[serde(default)]` for the one-shot migration.
  Pinned by `splat_config_skips_serialization` +
  `legacy_splat_config_round_trip_skips_serializing_after_migration`.

**Alternatives:**

- *Composite blend modes beyond `Normal`*. Considered (Multiply /
  Screen / Overlay are common). Deferred — the `BlendMode` enum +
  ComboBox surface ship empty so Sprint 18+ slots them in cheaply.

- *Pen-pressure painting*. Considered. Deferred — egui 0.33 doesn't
  surface tablet pressure natively. Sprint 20+ when a wrapper crate
  is wired.

- *Live full-resolution composite preview at > 4096²/axis*. Stays
  capped per ADR-039. The CPU bake at `.sd7`-build time is the
  authoritative full-res output.

- *Garbage collection of orphaned imported textures*. Considered.
  Undoing an import leaves the on-disk PNG behind (the layer's
  `Source` reverts but the file stays). Sprint 18 can add a
  "Sweep unused imports" affordance; not a Sprint 17 deliverable.

- *Multi-select layer ops*. Considered (Photoshop has it).
  Deferred — single-active model fits the current pointer-dispatch
  shape. Sprint 19+.

- *Layer groups / folders*. Deferred — most BAR maps land ≤ 6
  layers; groups would be over-design.

**Consequence:**

- `Tool::ALL` shrinks 10 → 9 (no more `SplatPaint`). Keyboard `T`
  is freed.
- `App` loses six fields (`splat_config`, `splat_distribution`,
  `splat_brushes`, `splat_brush_state`, `splat_picker_open_for`) +
  ~700 LoC of `inspector_splat` + `apply_splat_brush_at` +
  helpers.
- `barme-pipeline` gains `LayerSplatBakeInputs`, `LintWarning`,
  `stage_splat_assets_from_layers`, `materialize_splat_distribution_from_layers`,
  `populate_resources_from_layers`. The legacy entries
  (`stage_splat_assets`, `compute_active_channels`,
  `write_splat_distribution_png`, `populate_resources`,
  `SplatBakeInputs`) stay one more sprint as dead-callable code;
  Sprint 18 polish sweep.
- Tests: 264 in `barme-core`, 240 in `barme-app`, 117 in
  `barme-pipeline`. The mask-undo + DNTS-hybrid + slot-picker
  surfaces are pinned by 11 net new tests across the three crates.
- The user's 2026-05-19 "the textures of the end map are quite
  incredibly ugly" report closes: unlimited stylistic layers
  compose into the diffuse BMP at full resolution; bottom 4
  DNTS-bound layers drive runtime per-fragment normal mapping in
  BAR; the legacy 4-channel inspector is gone; stock textures are
  one click away (no upload required).

**Amendment 2026-05-21 (Sprint 23 / T1) — three followups closed:**

1. **16-SMU `Tool::PaintLayer` entry OOM (root cause).** Two
   contributors confirmed by `barme_core::rss` profile data:
   - (H1+H4) `render::ensure_composite_rt` zero-filled the
     16-layer mask array via a 65 536-call row-by-row
     `queue.write_texture` loop. wgpu zero-init defaults handle
     the case without any explicit writes; the loop was wasted
     staging-arena work that ballooned on iGPU shared memory.
     Fix: delete the loop.
   - (H2) `TileGrid::filled` unconditionally seeded
     `current_version = 1`, causing the first
     `App::sync_composite_mask_tiles` to upload every tile of
     every layer (~256 MB on a 4-layer 16-SMU project). Fix:
     seed the version at 0 when `fill == 0` so the CPU matches
     the GPU's zero-init default.
   - H3 (per-layer PNG decode on PaintLayer entry) refuted — the
     decode lives on project open / wizard / migration paths,
     not on tool entry.
   - Combined: entry-frame mask transfer drops 256 MB → 64 MB.
     Pinned by Linux-only `procfs`-backed RSS harness +
     CPU-side budget tests in `barme_core::layers::tests`.
     `trace!(target: "barme::rss", …)` snapshots fire at paint-
     stroke boundaries for ongoing observability.

2. **Orphan imported-texture GC.** New `barme_core::layers::
   garbage_collect_textures(project, project_root) ->
   io::Result<GcReport>` walks `<root>/textures/{*.png,
   *.meta.toml}`, computes the in-use UUID set from
   `LayerSource::imported_uuid()` (new accessor) across every
   layer, and unlinks orphans. `GcReport` carries
   `orphans_removed: Vec<PathBuf>` + `orphans_removed_bytes: u64`
   + `orphans_in_use_count: usize` + `errors: Vec<(PathBuf,
   io::Error)>`. Wiring:
   - Auto-runs after every successful save via `App::save_to`
     (silent when the report is empty; surfaces freed-MB total
     on `last_error` otherwise).
   - Manual `File > Garbage collect orphan textures` menu item
     (also surfaces "no orphans found" so the user knows the
     click did something).
   - Deliberately **does NOT** run on `ProjectDiff::RemoveLayer`
     — the 100 MB undo ring may still hold a snapshot of the
     removed layer with its source path, and Ctrl+Z within the
     undo window expects the file to be on disk. ADR-033
     eviction-driven GC is a follow-up.

3. **Legacy `SplatConfig` retired at runtime.** The struct
   itself (`crates/barme-core/src/splat.rs::SplatConfig`),
   `Project.splat_config` field, `LayerStack::migrate_from_
   splat_config` all deleted. Legacy `.barmeproj` loads now
   flow through `barme_core::layers::legacy_splat_config_to_
   layers(value: &toml::Value, size: MapSize) -> LayerStack`,
   which reads the on-disk `[splat_config]` block as a
   `toml::Value` and rebuilds the layer stack without a typed
   struct. `Project::after_load_migrate` takes the raw TOML
   text and returns `bool` (true when the migration fired),
   so `App::open_from` can surface a one-time terminal banner.
   `Project::load_from_file_reporting_migration` is the new
   raw-aware loader; `load_from_file` keeps its single-return
   shape and runs the migration with a `NullSlotResolver`.
   `SCHEMA_V` not bumped — serde's default-when-missing handles
   the field's absence on load (ADR-041 Commit 6 already
   shipped `#[serde(skip_serializing)]`). Compatibility verified
   against v=0 / v=1 / v=2 fixtures. The legacy splat pipeline
   path (`stage_splat_assets` / `compute_active_channels` /
   `populate_resources`) now derives its data from
   `project.layers.dnts_layers()` + `project.dnts_diffuse_in_alpha`
   instead of the deleted `splat_config`; the smoke binary's
   `layer_inputs = None` codepath continues to work because
   `Project::new` seeds a default biome layer that isn't
   DNTS-bound, leaving `compute_active_channels` returning
   `[false; 4]` and the legacy path a clean no-op.

After this sprint, the painter is **production-ready for 16-SMU
mappers** with no known memory leaks and a stable disk footprint.
Sprint 24 = multithreading (rayon procgen + parallel DNTS bake).

Critical files:
- NEW `crates/barme-app/src/ui/layers_panel.rs`
- `crates/barme-app/src/ui/widgets.rs` — `slot_picker_grid` +
  `SlotPickerEntry`.
- `crates/barme-app/src/ui/paint_view.rs` — `mask_only_preview`
  overlay render.
- `crates/barme-app/src/main.rs` — net −800 LoC (panel relocation
  + inspector_splat deletion + retirement of splat-painter App
  fields; offset by new helpers `add_layer_with_slot`,
  `layer_thumbnail`, `active_mask_overlay_texture`,
  `migrate_imported_layer_paths`, `import_layer_texture` rewrite).
- `crates/barme-core/src/layers/mod.rs` — `dnts_layers()` accessor,
  `dnts_tex_scale` + `dnts_tex_mult` fields, migration extension.
- `crates/barme-core/src/layers/mask.rs` — `pub Tile`,
  `clone_tile` / `restore_tile` / `tile_coords_overlapping_rect` /
  `brush_bbox`.
- `crates/barme-core/src/undo.rs` — `HistoryEntry::Mask`,
  `MaskEntry`, `OpenMaskStroke`, `snapshot_mask_tile` /
  `end_mask_stroke` / `mask_stroke_open`,
  `LayerPropertyValue::DntsTexScale` + `DntsTexMult`.
- `crates/barme-core/src/project.rs` — `Project.dnts_diffuse_in_alpha`,
  `Project.migration_toast_dismissed`, `splat_config`
  `#[serde(skip_serializing)]`.
- `crates/barme-pipeline/src/splat_pipeline.rs` —
  `stage_splat_assets_from_layers`, `materialize_splat_distribution_from_layers`,
  `populate_resources_from_layers`, `LayerSplatBakeInputs`,
  `LintWarning`.
- `crates/barme-pipeline/src/lib.rs::build_sd7` — `layer_inputs:
  Option<LayerSplatBakeInputs>` parameter + dispatch branch.

## ADR-043 — Unified terrain shader: line-by-line port of SMFFragProg.glsl (Sprint 25 / R1)

**Status:** Accepted 2026-05-21. Supersedes ADR-036 (the Sprint 9 / D4
diffuse-only splat composite). Opens the **renderer-parity arc**
(Sprints 25-36, per
`docs/research/renderer-bar-parity/ROADMAP.md`). After SRS §2.1 #11
was reversed on 2026-05-18, the editor's terrain renderer must
visually reproduce Recoil's render at editor camera distances. This
ADR pins the architecture for the fragment stage; Sprints 26-35 add
water polish, atmosphere, shadows, features, grass, emission, and
parallax on top of the same shader contract.

**Context.** The renderer-bar-parity ROADMAP authored 2026-05-18 (the
day the §2.1 #11 reversal landed) sketched a 9-sprint arc starting
with "Sprint 16 — Terrain shader parity". Planner-arc renumbering
since then moved that work to **Sprint 25 / R1**. ADR-036 — the
Sprint 9 / D4 splat-blended diffuse composite — was always an
explicit simplification (diffuse-only; no normal mapping, no
per-fragment specular, no base-normal sampling); the engine's
`SMFFragProg.glsl` `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` branch is
the real spec. FINDINGS §7.1–§7.6 (source-audit 2026-05-18) corrected
five inherited claims about the composite math (constant name typo,
TBN derivation, base normal R+A decoding, specular exponent formula,
alpha-sign interpretation); this sprint is the first to implement the
corrected version end-to-end.

Sprint 17 (ADR-041) retired the Sprint 9 slot-DIFFUSE array role
when the layered painter's composite RT took over as the diffuse
base. That left the 4-layer texture array (binding 5) bound but
unused at runtime. ADR-043 repurposes the binding as the
**DNTS slot NORMAL array** — the same 4 slots the engine binds at
`splatDetailNormalTex1..4`.

**Decision.**

1. **The fragment stage is a transcription, not interpretation.**
   Each WGSL section in `crates/barme-app/src/terrain.wgsl` cites
   the source GLSL line in `SMFFragProg.glsl` plus the FINDINGS
   subsection it implements. A future re-audit can read the WGSL
   alongside the GLSL line-by-line.

2. **Texture bind order** (Group 0, one bind-group layout — packing
   into the wgpu `MAX_BIND_GROUPS_PER_PIPELINE = 4` cap):

   | Binding | Resource                          | Engine analogue              | FINDINGS |
   |---------|-----------------------------------|------------------------------|----------|
   | 0       | terrain `Uniforms`                | engine uniform block         |          |
   | 1       | heightmap `texture_2d<u32>`       | (synth — engine reads at vs) |          |
   | 2       | `SplatU` uniforms                 | engine uniform block         |          |
   | 3       | splat distribution                | `splatDistrTex`              | §7.3     |
   | 4       | splat distribution sampler        |                              |          |
   | 5       | **slot normals texture array** (4 layers) | `splatDetailNormalTex1..4` | §7.3 |
   | 6       | slot normals sampler (Repeat)     |                              |          |
   | 7       | composite RT                      | (Sprint 16 — diffuse base)   |          |
   | 8       | composite sampler (Clamp)         |                              |          |
   | 9       | base normal map                   | `normalsTex`                 | §7.5     |
   | 10      | base normal sampler (Clamp)       |                              |          |
   | 11      | specular map                      | `specularTex`                | §7.6     |
   | 12      | specular sampler (Clamp)          |                              |          |

   Total: 13 bindings in one group. Within the per-group cap on every
   wgpu backend we ship to.

3. **Uniform block extension.** `SplatUniforms` grows two vec4s:
   - `ground_specular`: `.xyz` = fallback colour, `.w` = global
     exponent — both consulted only when no specular texture is
     bound (FINDINGS §7.6).
   - `camera_pos`: `.xyz` = world eye for the Blinn-Phong half-
     vector. The engine builds this in `SMFVertProg.glsl:34-41` but
     our vertex shader doesn't carry it; we hoist to per-frame
     fragment uniform via `TerrainCallback::camera_pos`.

   `flags.w` repurposed as a 3-bit texture-presence bitfield:
   - bit 0 = has_base_normal_tex
   - bit 1 = has_specular_tex
   - bit 2 = has_dnts_slot_normals (engine's
     `SMF_DETAIL_NORMAL_TEXTURE_SPLATTING` gate per
     `SMFRenderState.cpp:114` — NOT AND-ed with specular)

4. **GLSL → WGSL line mapping** (the core transcription
   correspondence):

   | GLSL line                | WGSL section                            |
   |--------------------------|------------------------------------------|
   | 146-150 `GetFragmentNormal`     | §2 base normal sample + R+A decode |
   | 174-198 `GetSplatDetailTextureNormal` | §4 DNTS composite + clamp .y, §7.3 |
   | 189 `.y = max(.y, 0.01)` | §4 clamp .y                              |
   | 192 `clamp(splatDetailNormal.a, -1, 1)` | §4 detail_strength_y       |
   | 205-206 `groundShadeInt`  | §7 shade_int (pre-dimmed CPU-side, §7.1) |
   | 269 `normal = GetFragmentNormal(...)` | §2 normal mix              |
   | 276-278 TBN matrix       | §3 per-fragment TBN, §7.4                |
   | 328 mix(normal, ...)      | §5 final normal blend                   |
   | 333-334 cosAngleDiffuse / cosAngleSpecular | §7 Lambert + Blinn-Phong |
   | 381 fragColor compose    | §8 final compose                         |
   | 404-422 specular branch  | §7 spec_col / spec_exp / spec_pow, §7.6  |

5. **Fallback textures.** Each of the new texture bindings gets a
   1×1 fallback so the bind group never changes shape per frame:
   - base normal default = `(128, 0, 0, 128)` — R+A decode produces
     `(0, sqrt(1), 0)` = pure up; the vertex normal carries the
     real signal when no real bake is loaded.
   - specular default = `(128, 128, 128, 64)` — mid-grey with
     exponent ≈ 4 (matte).
   - slot normals default = 4 layers of `(128, 128, 255, 128)` —
     the `* 2 - 1` decode produces `(0, 0, 1, 0)`, contributing
     nothing through `cofac × decoded`.

   The `flags.w` texture-presence bits are 0 by default; the shader
   mixes between sampled and fallback using uniform-controlled
   factors so WGSL doesn't flag non-uniform control flow.

6. **Pre-applied `SMF_INTENSITY_MULT = 210/255`.** Per FINDINGS §7.1,
   the engine multiplies `(ambient + diffuse * NdotL)` by this
   constant inside `GetShadeInt`. We pre-multiply
   `ground_ambient` + `ground_diffuse` CPU-side in
   `default_ground_ambient()` / `default_ground_diffuse()` so the
   WGSL stays free of the per-fragment multiply. The constant is
   exposed publicly as `render::SMF_INTENSITY_MULT` for the parity-
   fixture loader.

7. **WGSL parse + validate at `cargo test` time.** A new test
   `terrain_wgsl_parses_and_validates` uses wgpu's re-exported
   `naga::front::wgsl::parse_str` + the full Naga validator. The
   shader is only compiled on the GPU at `create_shader_module`
   time (which we can't reach in headless CI); this test catches
   WGSL syntax / type / binding-layout drift before the user runs
   the app. Failure messages emit Naga's source-line diagnostic.

**Alternatives considered.**

- *Keep ADR-036's diffuse-only shader and add normals as a separate
  pass.* Rejected — two shaders sampling the same heightmap doubles
  the draw + uniform overhead with no architectural benefit; the
  engine itself runs a single fragment shader.
- *Bake the heightmap-derived base normal CPU-side now.* Deferred —
  the vertex normal is good enough as a fallback for Sprint 25's
  acceptance criteria; the bake adds CPU work that's better done
  alongside Sprint 30's shadow map (both feed the same TBN-using
  surface model). The base-normal binding stays plumbed so the
  bake can drop in without a uniform-layout change.
- *Pack the 4 DNTS slot normals as 4 separate textures (engine's
  approach).* Rejected — texture arrays let one `textureSample(...,
  layer)` call replace 4 separate samplers, halving the bind-group
  cost.
- *Sample specular through a uniform-flagged `if`.* Rejected per
  pitfall #10 — uniform-controlled `mix` is cleaner and matches the
  WGSL spec's preferred pattern. The two-sample cost on a 1×1
  fallback is negligible.

**Consequence.**

- `crates/barme-app/src/terrain.wgsl` is the canonical port; future
  per-feature sprints (water, atmosphere, shadows, features, grass,
  emission, parallax) extend this same shader rather than spawning
  parallel WGSL files (one shader per terrain pass).
- ADR-036's "diffuse-only" gating disappears from the codebase.
  Historical comments in `barme-core::splat` and `ui::minimap` that
  cite "D5 / Sprint 9" remain accurate dev-log traces.
- `render::SplatResources` field rename: `slot_array_*` →
  `slot_normals_*` (the binding's new role). Sprint 9 / D4's
  `upload_diffuse_layer` → `upload_slot_normal_layer`.
- `render::TerrainCallback::new` now extracts `camera.eye()` and
  injects it into `SplatUniforms.camera_pos` at `prepare()` time.
- The renderer-bar-parity ROADMAP originally reserved ADR-038 for
  this work; the Sprint 15 layered-painter trio claimed that number
  earlier in 2026-05. We use **ADR-043**, the next free slot. The
  renderer-arc's later ADRs slide accordingly (atmosphere = 044,
  water polish folds into the existing ADR-042 amendment, shadows =
  045, etc. — to be pinned as each sprint opens).

**Deferred items** (each gets its own ADR when its sprint opens):

- **Sprint 26** (water polish: fresnel + foam + caustics + perlin +
  refraction + reflection) — ADR-042 amendment.
- **Sprint 28** (atmosphere + fog + sun + sky / skybox).
- **Sprint 29** (S3O / 3DO features as Sprint 11 / C5 markers retire).
- **Sprint 30** (directional shadow map).
- **Sprint 34** (grass blade instancing).
- **Sprint 35** (emission + sky-reflect + parallax).
- **Sprint 36** (parity validation: ΔE harness vs BAR reference).
- Heightmap → R+A base normal bake.
- Per-slot DDS / normal-PNG upload from the layer-stack DNTS bind
  (currently the slot-normal array stays at the 1×1 "flat-up"
  fallback because no upload path exists yet).
- ADR-034 high-pass diffuse-alpha workflow — the `diffuse_in_alpha`
  flag stays plumbed and the WGSL implements the `splat_detail_normal
  .a` path; runtime activation waits on the high-pass DNTS bake.

## ADR-044 — Water polish: BumpWater port + reflection / refraction (Sprint 26 / R3)

**Status:** ADOPTED 2026-05-21. Amends ADR-042 (Sprint 14 / C9, water +
lava as a map property) by replacing its renderer MVP — a flat alpha-
blended quad at `Y = 0` — with the renderer-parity arc's R3 sprint,
which lands a port of `BumpWaterFS.glsl` to the editor's wgpu pipeline.
Closes the polish deferral the ADR-042 STATUS UPDATE flagged: "Polished
water (fresnel, foam, caustics, lava emission, perlin wave motion) is
the renderer-parity arc's job."

**ADR numbering correction.** The Sprint 26 prompt called this ADR-039
("the polish ships as ADR-039 (next number after Sprint 25's
ADR-038)"). Inspection of `docs/DECISIONS.md` shows ADR-039 was already
taken by Sprint 16's GPU layered composite + tiled-COW mask storage,
and ADR-043 by Sprint 25's terrain shader parity. The next free slot
is **ADR-044**, and we ship under that number with a kickoff-log note
explaining the divergence (CLAUDE.md house rule #1 — surface spec
contradictions, don't silently work around them).

**Context.** Sprint 14 / C9 / ADR-042 shipped a water authoring flow
end-to-end: `WaterMode` preset enum, sparse `Project.water_overrides`,
Tool::Water inspector with five sections, lava-atmosphere offer, and
emission into `mapinfo.water`. The renderer cut was deliberately
minimal — one tinted quad with depth-test on, depth-write off,
premultiplied alpha — so the data path could land without blocking on
shader work. Sprint 26 closes the renderer gap.

The Sprint-25 / R1 / ADR-043 terrain shader (`SMFFragProg.glsl` port)
is the foundation: it already produces the realistic ground colour the
water shader needs to refract through. The remaining gap is the water
shader itself, which in the engine reads from a 350-line GLSL file
covering normal mapping, perlin waves, screen-space refraction, planar
reflections, foam, caustics, fresnel composite, and lava emission.

**Decision.** Eight tightly-coupled changes:

1. **Planar reflection pass.** Fixed `REFLECTION_RT_SIZE = 1024²`
   colour + depth RT allocated once at install time. A second terrain
   pipeline `pipeline_reflection` (front-face cull to compensate for
   the mirrored-Y winding flip) renders the terrain into the
   reflection RT with `OrbitCamera::view_proj_matrix_reflected_y0` —
   eye and target Y negated, up vector flipped. Gated on
   `App.water_reflections` (default ON; per-session preference, not
   persisted) AND `water.is_some()` so the cost is zero when water is
   off or the user disables reflections on iGPU.

2. **Refraction copy + render-pass split.** Sample-from-and-write-to
   of the same texture is UB on Vulkan / D3D12. The engine's
   `BumpWater.cpp` does a framebuffer copy before the water pass;
   we do the same via `encoder.copy_texture_to_texture(offscreen.color
   → refraction_copy)`. The single offscreen render pass that shipped
   in Sprint 14 splits into: Pass 1 (terrain) → COPY → Pass 2 (water +
   lines + markers with LoadOp::Load on colour and depth). The water
   shader's binding 1 reads from `refraction_copy`; perturbed
   screen-space UV gives the under-water distortion.

3. **WGSL fbm surface normal.** The engine samples a 512² normal map
   (`waterbump.png`) four times at progressively offset UVs combined
   with amplitudes `1, a, a², a³, a⁴` — morally fbm. We synthesise
   the same field in WGSL via 4-octave gradient noise (Quilez 2D hash,
   quintic-smoothed bilerp, 3.0-lacunarity). Tradeoff: we ship no
   binary asset (the engine's `waterbump.png` is GPL-2.0 inherited
   from springcontent), and the noise is deterministic across
   backends (Vulkan / Metal / D3D12 all produce identical pixels per
   `naga::front::wgsl` validation). Sprint 27 may revisit if visual
   parity demands the real texture.

4. **Fresnel composite.** Schlick's approximation:
   `fresnel = min + max × pow(1 - cos(view, normal), power)`. World-
   space camera eye threaded through `WaterU.eye` (added in commit 4).
   `dot` clamped to `[0, 1]` to dodge backface NaN (PITFALL #6 from
   the prompt). `polish_c.x/y/z` carry the three knobs; engine
   defaults 0.2 / 0.8 / 4.0 (FINDINGS §1.5).

5. **Foam (refraction-brightness proxy).** The engine's foam reads a
   precomputed coastmap (`BumpWaterCoastBlur*.glsl` produces a
   per-pixel distance-to-shore texture). The editor doesn't run that
   bake yet — explicitly Sprint 27 / R4 candidate. As a proxy we use
   the brightness of the refraction sample: bright refraction ≈
   shallow terrain ≈ near shoreline → foam. Imperfect but visually
   plausible at editor camera distances. The smoothstep band width
   comes from `polish_b.w = foam_height`.

6. **Caustics.** Procedural two-axis sine pattern warped by
   `surface_normal.xz`, animated by `polish_a.z = time_s`. Modulated
   by the same refraction-luma proxy so caustics only shimmer in
   shallow water (matches engine's `if (waterdepth > 0)` at
   `BumpWaterFS.glsl:325`).

7. **Lava emission glow.** When `water_mode ∈ {Lava, Magma}`,
   `polish_c.w = 1.0` gates a self-illumination branch:
   `emission_color × strength × (1 + caust · 0.5) × daylight`.
   `daylight` is hardcoded `0.5` until Sprint 28 wires
   `dot(sun_dir, world_up)`. Gating via multiply, not a divergent
   `if`, so the shader stays uniform across preset switches.

8. **Inspector polish section.** Collapsible "Polish" added to
   `inspector_water` below "Flood": reflections toggle + fresnel
   triple + reflection distortion + perlin start freq + lacunarity.
   Each control carries an `on_hover_text(HelpId::*)` per Sprint 19
   convention. Seven new HelpId variants.

**Alternatives considered.**

- **Vendor `waterbump.png` + `caustics/*.jpg` from the springcontent
  base.** Rejected — both are GPL-2.0 inherited; the editor's binary
  distribution would inherit the licence with material downstream
  implications. Procedural synthesis matches the engine's visual
  contract well enough at editor camera distances.

- **Two separate uniform buffers vs. ping-pong RTs for refraction.**
  The two-RT pingpong would mean rendering terrain to RT_A, water
  reading RT_A and writing RT_B, then blitting RT_B → RT_A for the
  next frame. Rejected — adds a per-frame allocator + blit; the
  texture-copy approach is the BAR engine's well-trodden shortcut
  and costs one full-RT memcpy on the GPU (~0.1 ms at 2048²) which
  is acceptable.

- **Render the reflection at the same resolution as the main RT.**
  Rejected — doubles the terrain-pass cost. Half-res (1024²) keeps
  the doubled-terrain cost bounded on Vega 8 iGPU (~1 ms reflection
  + ~3 ms main = under 16 ms frame target with headroom). The water
  shader perturbs the reflection UV anyway; minor pixel loss is
  invisible.

- **Sky cubemap reflections (water reflects the sky).** Deferred to
  Sprint 35 (emission + sky-reflect + parallax) — needs a skybox
  cubemap which the renderer doesn't bind yet.

- **Coastmap bake (proper shoreline foam).** Deferred — needs a
  pre-process pass equivalent to `BumpWaterCoastBlurFS` running at
  project load / heightmap edit. Adds CPU cost during sculpt; we
  ship the brightness-proxy foam now and revisit if the visual
  divergence is rejected.

- **Promote `wind_speed`, `wave_offset_factor`, `caustics_resolution`,
  `caustics_strength`, `wave_foam_distortion` to `WaterBlock`.**
  FINDINGS §1.5 lists them as engine-read Lua keys, but the editor's
  `WaterBlock` doesn't carry them yet. Promoting requires touching
  `barme-core::mapinfo_schema`, `barme-pipeline::mapinfo` emit, the
  F9 form (Sprint 18), and a fresh round of preset tuning. Sprint 26
  ships them as compile-time constants in `water_draw_for_frame`;
  the schema lift is a Sprint 27 / R4 backlog item documented here.

**Consequences.**

- The 3D preview's water surface now reproduces BAR's `BumpWaterFS`
  for the fields the editor's `WaterBlock` carries today (fresnel,
  perlin, surface tint, alpha, reflection-distortion). Wave shape,
  caustic brightness, and shore-foam are visually close but not
  pixel-perfect against the engine's preset-tied bake (Sprint 36
  parity validation will quantify the ΔE).

- The water shader's bind group expands from 1 binding (uniform) to
  5 (uniform + refraction tex/sampler + reflection tex/sampler).
  Bind group rebuilds on offscreen RT resize via the new
  `RenderResources::rebind_water`.

- `WaterU` grows from 96 to 192 bytes. The size + round-trip tests
  pin the layout; future polish parameters can extend the four
  `polish_*` vec4s without re-spinning the test scaffolding.

- The reflection pass adds ~1 ms terrain-pass cost on Vega 8 iGPU
  (half-res mitigation). The `App.water_reflections` toggle lets
  the user disable for low-end hardware; the inspector surfaces it
  with the disable-cost callout.

- 1 new naga validation test (`water_wgsl_parses_and_validates`)
  catches WGSL syntax / binding-layout drift at `cargo test` time
  — parallels Sprint 25's `terrain_wgsl_parses_and_validates`.

- 2 new camera-math tests pin `view_proj_matrix_reflected_y0`'s
  geometric contract (eye/target/up flip; above-Y reflected projects
  equivalently to below-Y normal).

**Pitfalls (operational notes for follow-up sprints).**

- The reflection pipeline uses `Face::Front` cull, NOT `Face::Back`.
  The mirrored-Y view flips winding; pairing `FrontFace::Cw` with
  `Face::Front` keeps the same visible-from-above triangles after
  the mirror. Sprint 27+ work that touches the terrain pipeline must
  apply changes to both `pipeline` and `pipeline_reflection`.

- Two uniform buffers, not one. `queue.write_buffer` collapses to the
  latest value before any encoder command runs; writing
  `mirrored_view_proj` then `main_view_proj` to one buffer would
  race the reflection pass. The split is intentional and load-bearing.

- The foam proxy will fail on very dark diffuse surfaces (e.g. a
  black-tinted ocean). Sprint 27 / R4 should ship the coastmap bake
  to fix this — drop a `bool has_coastmap` flag in the WaterU and
  branch the foam path.

- The lava-emission daylight factor is hardcoded `0.5`. When Sprint
  28 lands atmosphere + sun direction, replace the constant with
  `dot(normalize(sun_dir.xyz), vec3<f32>(0, 1, 0))` and the visual
  ramps from "max brightness at night" to "dimmed under direct sun"
  automatically.

- Shadows (Sprint 30) MUST inhibit the lava-emission branch under
  cast shadows. Today's code is shadow-free; the branch is
  `lit = false` semantically — add a shadow sample multiplier next
  to the daylight factor when the shadow map lands.

- The procedural caustics ship at a fixed two-octave sine pattern.
  Sprint 35 (parallax + emission + sky-reflect) may revisit if the
  visual is rejected; the engine's 32-image cycle is a strict
  improvement but needs ~6 MB of vendored caustics jpgs.

**Reference.** Engine shader:
`/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/BumpWaterFS.glsl`.
Engine state: `rts/Rendering/Env/BumpWater.cpp`. Schema reference:
`docs/research/source-audit-2026-05-18/FINDINGS.md` §1.5. Research:
`devlog/research-water-lava/`. Sprint devlog:
`devlog/sprint-26-water-polish/`.

## ADR-045 — Atmosphere + fog: exponential height fog, sky-as-clear, deterministic wind, sun-angle ramp (Sprint 28 / R2)

**Status:** Accepted (2026-05-21).
**Context.** Sprint 25 (ADR-043) shipped the terrain shader; Sprint 26
(ADR-044) shipped the water shader. The remaining renderer-parity
items that compose on top of both surfaces — fog, sky, sun-angle
modulation, and wind — were the cheapest sprint on the
renderer-parity arc (per the prompt: "Most of the work is plumbing —
the shader math is one fog equation"). Sprint 28 lands all four; the
single non-trivial engineering surface (skybox cubemap loading) is
explicitly deferred.

**Forces.**
- The shader math is straightforward (one smoothstep + one mix per
  effect) but the binding surface is large — atmosphere data lives in
  `mapinfo.atmosphere`, sun direction in `mapinfo.lighting`, and both
  the terrain and water shaders need the same values. A single shared
  uniform block keeps the CPU mapping in one place and avoids drift.
- The Sprint 26 water shader stubbed `daylight = 0.5` with a comment
  pointing at Sprint 28; that placeholder needs to resolve to the
  real `dot(sun_dir, +Y)` ramp.
- The Sprint 26 water shader also hardcoded `wind = 0.05`; needs to
  be derived from `atmosphere.min_wind`/`max_wind`.
- BAR maps configure `atmosphere.sky_color` distinct from `fog_color`
  (sky is brighter; fog is duller). The terrain rasteriser doesn't
  fill the offscreen RT past the mesh edges, so the cleared
  background needs to be the sky tone, not the legacy navy.
- Skybox cubemap loading is the only meaningful engineering work
  this sprint *could* contain — a new pipeline, a `texture_cube<f32>`
  binding, a PNG-folder loader, a content-addressed cache. The user
  scope-decision (2026-05-21) defers it; this ADR captures the
  deferral so the next sprint doesn't have to re-litigate.

**Decisions.**

1. **`AtmosphereUniforms` is a sibling block to `SplatUniforms`, not
   an extension.** A new 144-byte (9 × vec4) repr(C) struct lives in
   its own uniform buffer, bound at terrain group 0 binding 13 AND
   water group 0 binding 5. SplatUniforms stays 128 B and continues
   to carry sun_dir for the terrain-specific Blinn-Phong path
   (cheaper than restructuring Sprint 25); the atmosphere block
   duplicates sun_dir so the water shader doesn't have to bind
   SplatUniforms. The duplication is intentional — one normalisation
   point CPU-side (`atmosphere_uniforms_for_render`), reads from the
   block stay cheap.

   **Alternative considered.** Extend SplatUniforms in-place. Rejected:
   breaks the Sprint 25 size-pin test (`SplatUniforms` is fixed at
   128 B for layout-drift detection); intermingles
   atmosphere-as-cross-shader-concern with splat-as-terrain-specific.

2. **Fog math: exponential height + smoothstep.** Following BAR's
   `Atmosphere.cpp::DrawFog`:
   `fog_t = smoothstep(fog_start, fog_end, dist_norm × exp(-y × falloff))`,
   then `mix(lit, fog_color.rgb, fog_t × fog_density)`. Height
   falloff (engine's `0.01..0.02`) thins fog at altitude — a mountain
   peak reads sharper than a valley floor at the same horizontal
   distance.

   **Alternative considered.** Linear fog (`clamp((dist - fog_start)
   / (fog_end - fog_start))`). Rejected: BAR uses exponential, parity
   is the sprint goal.

3. **Sky as clear-colour (not a dedicated pipeline).** The main
   offscreen pass's `LoadOp::Clear` reads from a runtime
   `atmosphere_clear_color(atmos)` instead of the constant
   `OFFSCREEN_CLEAR_COLOR`. The terrain rasteriser only covers ~80 %
   of the viewport at default framing; the cleared background fills
   the rest. This is the "no skybox" branch of BAR's
   `Sky.cpp::DrawSky` (which clears with `sky_color` when no cubemap
   loads).

   **Alternative considered.** Dedicated fullscreen-quad `sky.wgsl`
   pipeline with horizon gradient. Rejected for Sprint 28: the
   horizon gradient is only visually meaningful next to a cubemap
   (without it the gradient and the fog blend look like two competing
   horizon effects). The cubemap-aware pipeline lands with the
   deferred-cubemap sprint, at which point the gradient becomes
   load-bearing.

4. **Sun-colour angle ramp.** Replace Sprint 25's flat
   `sp.ground_diffuse` consumption with
   `mix(atmos.fog_color, sp.ground_diffuse, clamp(sun_dir.y, 0, 1))`.
   At low sun (horizon), lit terrain takes on the fog tint
   (sunset/sunrise glow). At zenith, full daylight. The water shader
   shares the same factor for its lava-emission daylight ramp:
   `daylight = pow(1 - clamp(sun_dir.y, 0, 1), 0.7)` — lava brightens
   at night, dims under direct sun. The `0.7` exponent keeps lava
   noticeable at twilight rather than collapsing to "only visible at
   midnight".

5. **Deterministic wind state — no seed.** `App::atmosphere_uniforms_for_render`
   computes wind as
   `magnitude = lerp(min_wind, max_wind, sin(time × 0.1) × 0.5 + 0.5)`,
   `angle = time × 0.0233 × TAU`, then `(wind_x, wind_z) = (cos(angle)
   × magnitude, sin(angle) × magnitude) × scale`. The slow ~43 s
   rotation period feels ambient. PITFALL #7 forbids seeded noise so
   parity fixtures reproduce byte-for-byte across runs.

   **Alternative considered.** Per-tick RNG (rand + per-project seed).
   Rejected: parity fixtures need bit-for-bit reproducibility, and a
   periodic sin/cos ramp visually matches the engine's wind feel.

6. **Skybox cubemap loading: deferred.** No `texture_cube<f32>`
   binding, no PNG-folder loader, no dedicated `sky.wgsl` pipeline,
   no content-addressed cache. `AtmosphereUniforms.flags[0] =
   has_skybox` stays 0 for Sprint 28; `sky_axis_angle` is plumbed
   through the uniform but unused. The future cubemap sprint extends
   binding 13 (terrain) / 5 (water) layouts with a sibling cubemap
   binding + sky pipeline; the uniform layout doesn't change.

   **Why defer.** The prompt itself called this "the heaviest
   engineering work." Shipping fog + sun ramp + sky colour + wind
   without cubemap still hits the ROADMAP's renderer-parity bullet
   (R2). The next sprint can land cubemap + horizon gradient in a
   focused pair of commits.

7. **`fog_start == fog_end` defensiveness.** Sprint 21 (C8) lints
   this as a hard error, so a saved project can't transit it. But a
   freshly-typed F9 form value can. The shader's `smoothstep`
   clamps to [0, 1] without producing NaN even at the degenerate
   input (returns 0 below the range, 1 above), so the worst case is
   a binary step instead of a smooth blend — visible but not a
   crash.

**Consequences.**

- `AtmosphereUniforms` (144 B / 9 vec4) lands in `render.rs` with a
  size-pin test, a defaults-match-MapInfo test, and a wind-determinism
  test. Both the terrain and water bind groups grow by one binding.
- `App::atmosphere_uniforms_for_render` is the single CPU mapping
  point — pulls from `Project → MapInfo`, normalises `sun_dir`,
  computes wind. Used twice per frame (once for the terrain
  callback, once inside `water_draw_for_frame`); ~no cost.
- `OFFSCREEN_CLEAR_COLOR` constant survives as the reflection-pass
  fallback only. The main offscreen clear is per-frame.
- Sprint 26's `daylight = 0.5` placeholder is replaced; lava reads
  correctly across BAR's day/night cycle.
- Sprint 26's hardcoded `wind = 0.05` is replaced; water motion now
  responds to the F9 form's `min_wind` / `max_wind` sliders.
- 4 new tests (size pin, default-match-MapInfo, MapInfo round-trip
  through `atmosphere_uniforms_for_render`, wind determinism). Both
  shader naga validators pass.
- Parity fixtures (`foggy-map`, `sunset`) exercise the fog and the
  sun-angle ramp respectively. Skybox fixture deferred to the next
  sprint.

**Pitfalls (operational notes for follow-up sprints).**

- Cubemap loading sprint MUST keep `AtmosphereUniforms` at 144 B
  (or extend deliberately + update the size-pin test). The
  cubemap texture is a separate binding, not a uniform field.
- The reflection pass clear stays at the legacy navy
  `OFFSCREEN_CLEAR_COLOR`. Over-painting it with `sky_color` would
  mis-tint the water reflection sampler (the reflection RT is a
  "geometry only" scene, sampled by perturbed UV; we composite the
  sky on top of the main offscreen, not into the reflection RT).
- The shader's height-fog falloff coefficient (`0.01`) is hardcoded
  in `atmosphere_uniforms_for_render`. BAR's engine reads it from a
  map property the editor doesn't model yet; if a parity-validation
  test fails on a foggy-map fixture, the lift surface is small (one
  field).
- Skybox rotation (`sky_axis_angle`) is plumbed through but
  unconsumed. The deferred-cubemap sprint adds the Rodrigues-formula
  `rotate_axis_angle` helper and feeds the rotated direction into
  the cubemap sampler.
- `cloud_density` ships in the uniform but the shader doesn't
  modulate by it (flat sky this sprint). The deferred-cubemap
  sprint or Sprint 35 (parallax + emission + sky-reflect) can wire
  it.
- `atmosphere.sun_color` (the engine's "sun disc" tint) is NOT the
  ramp's zenith colour — that's `lighting.ground_diffuse`. The
  CPU mapper deliberately reads from `lighting` not `atmosphere` for
  this field. A future sun-disc rendering pass will read from
  `atmosphere.sun_color`.

**Reference.** Engine shader:
`/home/teague/code/RecoilEngine/cont/base/springcontent/shaders/GLSL/Atmosphere.glsl`,
`Sky*.glsl`. Engine state: `rts/Rendering/Env/Atmosphere.cpp`,
`SkyLight.cpp`, `Sky.cpp`. Schema reference:
`docs/research/source-audit-2026-05-18/FINDINGS.md` §1.3. Sprint
devlog: `devlog/sprint-28-atmosphere-and-fog/`.

## ADR template

```
## ADR-NNN — One-line decision

**Status:** Proposed | Accepted | Superseded by ADR-XXX
**Context:** Why we're deciding this now; what forces are at play.
**Alternatives:** What we considered and rejected, with one-line rationale.
**Consequence:** What changes in the code/process because of this.
```
