# BAR Map Editor

A standalone desktop GUI for authoring [Beyond All Reason] / Recoil maps.
It produces a playable `.sd7` from an empty project on Windows and Linux.
Single Rust binary (egui + eframe + wgpu); [PyMapConv] is bundled as a
sidecar for SMF/SMT compilation.

[Beyond All Reason]: https://www.beyondallreason.info/
[PyMapConv]: https://github.com/Beherith/springrts_smf_compiler

> **Status:** Stage 1 (MVP). See `docs/ROADMAP.md`. The canonical spec is
> `SRS.md`; agent/working notes are in `CLAUDE.md`.

## Platforms & releases

- **MSRV: Rust 1.90.** Dev toolchain pinned to stable 1.96
  (`rust-toolchain.toml`).
- **CI-tested on Linux + Windows + macOS** (`.github/workflows/ci.yml`).
- **Tagged releases ship** a Linux **AppImage** (`BARMapEditor-x86_64.AppImage`),
  a Windows **`.exe`** bundle, and an **experimental** macOS **`.app`**
  (unsigned — Gatekeeper will warn). See `.github/workflows/release.yml`.

NFR coverage now gated in CI: **performance** (brush-latency criterion
bench + ceiling test), **determinism** (byte-identical `.sd7`), and
**portability** (3-OS build matrix). See `CONTRIBUTING.md`.

## Build / run

```bash
. "$HOME/.cargo/env"          # rustup is user-local; or use ./scripts/dev.sh
cargo run -p barme-app        # launch the editor
cargo test --workspace        # unit + integration tests
```

To produce a Linux AppImage locally:

```bash
cargo build --release -p barme-app
./scripts/fetch-pymapconv.sh && ./scripts/fetch-compressonator.sh
./scripts/build-appimage.sh                 # lean image (no texture pack)
INCLUDE_TEXTURES=1 ./scripts/build-appimage.sh   # fully-offline image
```

## Repository layout

```
crates/
  barme-app/         egui UI shell, main entry point
  barme-core/        Project model, heightmap, brushes, procgen, undo
  barme-pipeline/    PyMapConv driver, mapinfo emit, .sd7 packaging
  barme-render-s3o/  CPU S3O thumbnail bake
docs/                ARCHITECTURE / ROADMAP / PITFALLS / DECISIONS
scripts/             dev wrapper + vendoring + build-appimage
tools/               vendored PyMapConv + Compressonator + textures (gitignored)
```

## License

GPL-2.0-or-later. Vendored PyMapConv + Compressonator carry their own
(CC0 / permissive) licenses — see `CREDITS.md`.
