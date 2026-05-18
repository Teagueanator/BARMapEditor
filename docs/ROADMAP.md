# Roadmap

Mirrors SRS §3.2 (functional reqs) and the staged plan in SRS "Recommendations".
Treat this as the *engineering* breakdown; the SRS is the *product* spec.

## Stage 0 — Validation prototype (target: 2 weeks) — ✅ COMPLETE 2026-05-17

Go/no-go gate before committing to the full build. If anything here proves
unviable, the SRS prescribes a fallback to Godot 4 + HTerrain.

- [x] Repo scaffolded (Rust workspace, two crates, docs)
- [x] Rust toolchain installed (rustup, stable 1.95)
- [x] `cargo check` clean on workspace
- [x] `cargo run -p barme-app` opens a window
- [x] Load a 16-bit PNG heightmap from `assets/fixtures/`
- [x] Render it as a meshed terrain via wgpu (single draw call, no LOD yet)
- [x] Serialize a project to TOML on disk, reload it
- [x] Vendor PyMapConv under `tools/pymapconv/` (also Compressonator under
      `tools/compressonator/` — see ADR-014)
- [x] Shell out to PyMapConv with a fake-project export → produce a valid `.sd7`
- [x] Launch BAR with that `.sd7` selected and verify it loads in-engine
- [x] Decision recorded in `docs/DECISIONS.md`: **PROCEED with Rust stack**
      (ADR-016)

Stage 0 surprises that informed Stage 1 scope (full list in ADR-016):
three-gate mapinfo model (engine / Chobby / mod gadgets each have
independent requirements); Compressonator is a separate vendor from
PyMapConv; PyMapConv v0.6.3 exits 1 on success and needs `-q 1` to dodge
a multi-thread read-back bug; Chobby filters unofficial maps out of the
multiplayer browser (Skirmish-only).

## Stage 1 — MVP (3–4 months)

Implements SRS F1–F12. Ships a Windows `.exe` and a Linux AppImage.

- [ ] **F1** New-project wizard (size, biome preset, symmetry mode)
- [ ] **F2** Real-time heightmap sculpting (raise / lower / flatten / smooth / erode / ramp)
- [ ] **F3** Symmetry enforcement (mirror H/V/diag/rot-N)
- [ ] **F4** Texture painting via DNTS splat channels (4 RGBA, ≤4 splat textures)
- [ ] **F5** Metal-spot placement (point + radius → red-channel density)
- [ ] **F6** Geo-vent placement
- [ ] **F7** Feature placement (trees, rocks, wreckage) into a Lua gadget
- [ ] **F8** Start-position editor
- [ ] **F9** `mapinfo.lua` editor (form + raw Lua tab)
- [ ] **F10** Minimap auto-generation
- [ ] **F11** One-click `.sd7` build via PyMapConv
- [ ] **F12** "Launch in BAR" button (invokes Recoil with `--map`)
- [ ] Beherith (or active mapper) reviews `.sd7` byte-for-byte against PyMapConv
      reference output on three test maps
- [ ] Listed on `beyondallreason.info/guide/mapmaking-resources` as beta

## Stage 2 — v1 (additional 4–6 months)

SRS F13–F17 plus quality-of-life.

- [ ] **F13** Decompile / import existing `.sd7`
- [ ] **F14** Procedural terrain (FBM, hydraulic erosion, river carve)
- [ ] **F15** Type-map editor + per-terraintype gameplay params
- [ ] **F16** Skybox picker / atmospheric preset library
- [ ] **F17** Pathability overlay
- [ ] "Lint My Map" pass — catches all ten silent `mapinfo.lua` pitfalls in
      `docs/PITFALLS.md`
- [ ] Procedural template library (Quicksilver, Glitters, Throne, Supreme
      Isthmus archetypes)

## Stage 3 — v2 (open-ended)

- [ ] **F18** DEM (GeoTIFF) import
- [ ] **F19** Procedural feature scatter with rule sets
- [ ] **F20** "Publish to BAR" — opens a PR against `maps-metadata` with
      generated YAML row

## Pivot thresholds (from SRS)

- PyMapConv stops being maintained, or licensing reverses → embed Rust-native
  SMF/SMT writer via `texpresso` / `bcdec`. +2 months.
- Recoil changes SMF format → embedded writer must follow.
- Brush latency on Intel iGPU > 16 ms at 32×32 → drop to CPU tile-update with
  coarser preview LOD.
