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

## Template for new entries

```
## ADR-NNN — One-line decision

**Status:** Proposed | Accepted | Superseded by ADR-XXX
**Context:** Why we're deciding this now; what forces are at play.
**Alternatives:** What we considered and rejected, with one-line rationale.
**Consequence:** What changes in the code/process because of this.
```
