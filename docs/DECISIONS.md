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

**Status:** Accepted (2026-05-17)
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

---

## Template for new entries

```
## ADR-NNN — One-line decision

**Status:** Proposed | Accepted | Superseded by ADR-XXX
**Context:** Why we're deciding this now; what forces are at play.
**Alternatives:** What we considered and rejected, with one-line rationale.
**Consequence:** What changes in the code/process because of this.
```
