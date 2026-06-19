# Contributing to BAR Map Editor

This file documents the CI gates and bench conventions introduced in
Sprint 33 (T6 / ADR-049). For *what* the project is and the architecture,
see `SRS.md`, `CLAUDE.md`, and `docs/`.

## Toolchain

- **MSRV: 1.90** — pinned in `Cargo.toml` (`[workspace.package]
  rust-version`). Don't use stdlib/edition features newer than 1.90
  without bumping the MSRV (with a note in `docs/DECISIONS.md`).
- **Dev stable** is pinned in `rust-toolchain.toml` (currently 1.96).
  This is the *upper* edge everyone formats/lints against; bump it as
  new stable releases land.
- CI runs a `rust: [stable, "1.90"]` matrix; both must `cargo check
  --workspace --all-targets`. The MSRV cell forces its toolchain with
  `RUSTUP_TOOLCHAIN` so `rust-toolchain.toml` doesn't override it.

## Before every commit

```bash
./scripts/dev.sh fmt --all
./scripts/dev.sh clippy --workspace --all-targets -- -D warnings
./scripts/dev.sh test --workspace
```

(`scripts/dev.sh` is a thin `cargo` wrapper that sources the user-local
rustup env.) CI enforces all three.

## Benchmarks

Perf-sensitive code (NFR-Performance ≤ 8 ms brush latency; the Sprint-24
≤ 250 ms procgen target) is guarded **two** ways:

1. **Criterion benches** (`crates/barme-core/benches/`) track the trend.
   CI runs them on a `perf` lane against a cached baseline and fails a
   PR on a **> 1.5×** regression. The 1.5× (not 1.0×) margin absorbs the
   noise of shared GitHub runners. Run locally:
   ```bash
   ./scripts/dev.sh bench -p barme-core --bench brush_latency
   ./scripts/dev.sh bench -p barme-core --bench procgen_apply
   ```
2. **CI-safe ceiling unit tests** assert a *generous* absolute bound that
   never flakes (e.g. `brushes::tests::smooth_stamp_16_smu_under_budget`,
   `procgen::tests::generate_16_smu_parabolic_parallel_under_400ms`).
   These run in the normal test lane.

The `examples/bench_*.rs` one-shot reporters are kept for human-readable
numbers; the criterion benches are the gated versions.

## Headless / GPU tests

CI has no real GPU. The `test` lane installs **Mesa Lavapipe** (software
Vulkan) + `xvfb` so wgpu-touching code can get a device. Lavapipe is fine
for *sanity*, **not** for pixel-parity validation (that's the Sprint-36
parity gate, which needs real hardware).

For a test that genuinely needs a GPU and can't run on a GPU-less runner,
gate it with the registered cfg:

```rust
#[test]
#[cfg_attr(ci_without_gpu, ignore)] // skipped where no device is available
fn needs_a_real_device() { /* … */ }
```

`ci_without_gpu` is registered in `Cargo.toml`
(`[workspace.lints.rust] check-cfg`) so it's lint-clean. CI sets
`RUSTFLAGS=--cfg ci_without_gpu` only on lanes without a device. Prefer
this one flag over scattering `#[cfg(target_os = "…")]` gates.

## `.sd7` determinism (NFR-Determinism)

The same project must compile to a byte-identical `.sd7`. Two guards:

- `sd7::tests::package_is_byte_identical_on_repeat` — packaging layer,
  needs only system 7z, runs in the normal CI lane. This is the live
  regression gate for the `-mtm- -mtc- -mta-` timestamp strip.
- `tests/sd7_determinism.rs` — full PyMapConv→pack pipeline, `#[ignore]`d
  (needs the vendored toolchain). Run manually:
  ```bash
  ./scripts/dev.sh test -p barme-pipeline --test sd7_determinism -- --ignored
  ```

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds on
ubuntu / windows / macos and attaches:

- `BARMapEditor-x86_64.AppImage` (Linux; `scripts/build-appimage.sh`)
- `BARMapEditor-windows-x86_64.zip` (Windows `.exe` bundle)
- `BARMapEditor-macos.zip` (`.app`, **experimental** — unsigned)

Packaged builds find their bundled `tools/` + `assets/` via the
`BARME_ROOT` env override (the AppImage's `AppRun` sets it). Dev builds
fall back to the workspace root automatically.

## Git workflow

Terse one-line commit subjects. **No** `Co-Authored-By` / "Generated
with" trailers. Run fmt + clippy + test before committing.
