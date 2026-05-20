# Sprint 33 — NFR / CI gates: MSRV matrix + brush bench + .sd7 determinism + Windows + AppImage (T6)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 33** — a **beta-prep** sprint. The 2026-05-20 SRS
audit found that ~6 NFR commitments have no CI test gate:

- **NFR-Performance** (≤8 ms brush latency at 16-SMU) — no
  regression bench in CI.
- **NFR-Determinism** (byte-identical `.sd7` output) — only
  per-emitter tests; no end-to-end build determinism test.
- **NFR-Portability** (Windows x86_64 + Linux x86_64) — Linux-only
  development; no Windows CI build, no AppImage build.
- **MSRV 1.90** — CLAUDE.md claims 1.90; the dev box runs 1.95.
  No `rust-toolchain.toml`, no MSRV CI matrix.
- **Headless test runner** — no Docker image, no `xvfb` / Mesa
  software-rendering fallback for the wgpu test that wants a
  surface.
- **Crash safety / log-collection on CI failure** — flaky tests
  give no log artifact for triage.

After this sprint, every NFR has a CI gate; we ship a Windows .exe
+ Linux AppImage on every tagged release; bench regressions fail
the PR; MSRV drift fails the PR.

**Prerequisites:**
- Sprint 32 (F12 + autosave) MUST be ticked. CI gates protect
  the now-stable MVP surface.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.3 (every NFR
   commitment).
3. `.github/workflows/` — current CI (if any). Likely sparse;
   this sprint fills it.
4. `/home/teague/code/BARMapEditor/Cargo.toml` (workspace) —
   add `rust-toolchain.toml` next to it.
5. Existing bench: `crates/barme-core/examples/bench_brushes.rs`
   (Sprint 1 / A1). Promote into a proper criterion bench.
6. Sprint 24 bench: `crates/barme-core/examples/bench_procgen.rs`
   (added in Sprint 24). Same promotion path.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-33-ci-gates
./devlog/log.sh new sprint-33-windows-appimage
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. `rust-toolchain.toml` + MSRV CI matrix

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.95"  # current stable
components = ["rustfmt", "clippy"]
```

`Cargo.toml` (workspace):
```toml
[workspace.package]
rust-version = "1.90"  # MSRV
```

CI matrix (`.github/workflows/ci.yml`):
```yaml
matrix:
  rust: [stable, 1.90]  # current stable + MSRV
```

Both must pass `cargo check --workspace`. If MSRV breaks, fix
the offending code OR bump MSRV (with documentation).

### 2. Criterion benches in CI

Promote `bench_brushes.rs` and `bench_procgen.rs` into proper
`criterion` benches. New deps:

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
```

New benches:
- `crates/barme-core/benches/brush_latency.rs` — brush smooth
  at radius 1024 on 16-SMU; assert <8 ms median.
- `crates/barme-core/benches/procgen_apply.rs` — 16-SMU
  parabolic apply; assert <250 ms median (Sprint 24 target).

CI runs the bench on a labelled "perf" lane. Compares against
a baseline stored at `.cargo-baseline/` (cached between runs).
Fails the PR if regression >1.5×.

### 3. `.sd7` determinism end-to-end test

`crates/barme-pipeline/tests/sd7_determinism.rs`:

```rust
#[test]
#[ignore = "needs pymapconv + 7z"]
fn sd7_byte_identical_on_repeat() {
    let project = fixture_project_minimal();
    let sd7_a = build_to_tmpdir(&project).unwrap();
    let sd7_b = build_to_tmpdir(&project).unwrap();
    let bytes_a = std::fs::read(&sd7_a).unwrap();
    let bytes_b = std::fs::read(&sd7_b).unwrap();
    assert_eq!(bytes_a, bytes_b, "build is non-deterministic!");
}
```

Run this in CI on the "integration" lane (already #[ignore]'d
for vendored toolchain availability). Surfaces PyMapConv-side
non-determinism if it ever creeps in.

### 4. Windows + macOS build

`.github/workflows/release.yml`:

```yaml
matrix:
  os: [ubuntu-latest, windows-latest, macos-latest]
```

Each builds `cargo build --release -p barme-app`. On tagged
release, uploads the binary as a release artifact.

**Windows-specific**: `barme-app.exe` plus the bundled
`tools/pymapconv/` + `tools/compressonator/`. Use the upstream
PyMapConv Windows binaries (already supported per Stage 0).

**macOS-specific**: untested before. May surface wgpu/Metal
issues. If too rough, ship as "experimental" with no auto-
release; document the gap.

### 5. AppImage build

`scripts/build-appimage.sh`:
- Bundles `barme-app` + `tools/pymapconv/*` + `tools/compressonator/*`
  + `tools/textures/` (the stock pack from Sprint 7 / D1) +
  required `.so`s into an AppDir.
- Runs `appimagetool` to produce `BARMapEditor-x86_64.AppImage`.

CI runs this on tagged release. Uploads as artifact.

### 6. Headless CI for wgpu-touching tests

Some tests want a wgpu device. Strategies:
- **Linux**: install Mesa Lavapipe (software Vulkan); set
  `WGPU_BACKEND=gl` or `VK_ICD_FILENAMES=$mesa_path`. CI
  `apt install mesa-vulkan-drivers libgl1-mesa-dri`.
- **macOS**: Metal works headless natively.
- **Windows**: D3D12 needs a real or virtual GPU; CI may skip.

Document the matrix and gate tests with
`#[cfg_attr(ci_without_gpu, ignore)]` annotations as needed.

### 7. CI log-collection on failure

On CI test failure, upload `cargo test --workspace -- --nocapture
2>&1 | tee test-output.log` as an artifact. Helps triage flaky
tests without re-running.

`.github/workflows/ci.yml`:
```yaml
- name: Upload test logs on failure
  if: failure()
  uses: actions/upload-artifact@v4
  with:
    name: test-logs-${{ matrix.os }}-${{ matrix.rust }}
    path: test-output.log
```

### 8. Documentation + rollup

- Add a `CONTRIBUTING.md` with the bench/CI conventions.
- README.md update: "MSRV 1.90; CI tested on Linux + Windows +
  macOS; tagged releases ship .exe + AppImage + .app bundle."
- ADR-042 (new): "Beta-prep CI gates" — minor.
- STATUS UPDATEs in SRS / ROADMAP (T6 done; NFR-Portability +
  NFR-Performance + NFR-Determinism honoured).
- "Sprint 34 = grass rendering" handoff.

## Step 4 — Standing constraints

Same as prior sprints. CI YAML changes are reviewed extra
carefully; a misconfigured workflow can mask real failures.

## Step 5 — Out of scope

- **Code coverage** (line/branch %) — defer; SRS doesn't commit
  to a number.
- **Fuzz testing** of the parsers — Stage-2 polish.
- **Benchmark regression alerting via Slack/Discord** — Stage-2.
- **Auto-publish to releases.beyondallreason.info** — Stage-3.
- **Cross-compilation from Linux** (Linux dev box producing
  Windows binary without Windows runner) — try `cross`; document
  if it works, but native runners are the default.

## Step 6 — Critical pitfalls (read twice)

1. **MSRV ≠ current stable**. `rust-toolchain.toml` pins current
   stable for dev; `rust-version` in Cargo.toml pins MSRV. The
   matrix tests both. If code uses a feature from stable that's
   not in MSRV, CI fails — fix or bump MSRV.

2. **Criterion bench instability on CI runners**: GitHub
   Actions runners are noisy. Fail only on >1.5× regression
   (not 1.0×). Report median of 5 runs.

3. **PyMapConv determinism**: timestamps in zip headers are a
   classic non-determinism source. The pipeline already passes
   `-mtime` to 7z (verify). If determinism still flakes, the
   issue is upstream PyMapConv; document with a flake-rate.

4. **Windows binary signing**: not in this sprint. Unsigned
   .exes trigger Windows SmartScreen. Document; Stage-2 polish
   addresses with a code-signing cert.

5. **macOS unattested before this sprint**: wgpu/Metal might
   surface bugs. If so, ship Sprint 33 with macOS marked
   experimental + a tracking issue.

6. **AppImage size**: bundling `tools/textures/` (~16 slots,
   ~100 MB) bloats the AppImage. Consider downloading textures
   on first launch instead. Decide at sprint kickoff.

7. **Mesa Lavapipe correctness**: software Vulkan may render
   slightly differently from real Vulkan. Acceptable for
   sanity tests; not for parity (Sprint 36) validation.

8. **CI cache invalidation**: GitHub Actions caches `target/`
   across runs. A stale cache can mask MSRV breaks. Add a
   cache-bust on `rust-toolchain.toml` change.

9. **`#[cfg_attr(ci_without_gpu, ignore)]` convention**:
   define `ci_without_gpu` in `Cargo.toml` features or via
   `RUSTFLAGS`. Don't proliferate `#[cfg(target_os = "X")]`
   gates; use the dedicated feature flag.

10. **Determinism test interaction with Sprint 18 minimap
    render**: the headless wgpu device's output may differ
    between runs on the same hardware due to driver
    nondeterminism. The minimap-render test may need to gate
    "byte-identical" to text files only (Lua, manifest) and
    treat PNG output as "structurally equal" (PSNR > 40 dB
    against a fixture).

## Step 7 — Exit criteria

- 6+ commits on `main`: toolchain + MSRV, benches, .sd7
  determinism, Windows + macOS build, AppImage, headless +
  log-collection, rollup.
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (T6 done; NFR-Portability +
  NFR-Performance + NFR-Determinism honoured).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green on all 3 OSes.
- Tagged-release CI produces `barme-app.exe`, `BARMapEditor-x86_64.AppImage`,
  + `BARMapEditor.app` (macOS, marked experimental if rough).
- Smoke test:
  - Local CI matrix run via `act` (or PR-trigger) → all matrix
    cells green.
  - Brush bench reports stable median across 5 runs.
  - AppImage executes on a clean Ubuntu 22.04 VM.
- Final devlog: summary + "Sprint 34 = grass rendering" handoff.

Start by writing `rust-toolchain.toml` + bumping the MSRV check
locally. Then the benches; then the Windows/macOS build matrix.
