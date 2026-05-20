# Sprint 43 — F16 Skybox picker + atmospheric preset library

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 43** — implements **F16** (skybox picker +
atmospheric preset library) from the SRS Stage-2 list. Today the
user can set `mapinfo.atmosphere.skyBox` to a file path via F9
form's Atmosphere tab. Sprint 43 ships:

1. **Skybox picker** — a gallery of curated CC0 skyboxes, vendored
   under `tools/skyboxes/`. Click a thumbnail to apply.
2. **Atmospheric preset library** — bundles of sun_dir + fog +
   sky_color + cloud + skybox into single-click presets ("Bright
   Day", "Sunset", "Overcast", "Night", "Alien Twilight", etc.).

After this sprint, mappers can iterate on the atmosphere without
touching individual fields.

**Prerequisites:**
- Sprint 28 (atmosphere + fog + cubemap loading) — the rendering
  backbone.
- Sprint 32 (autosave / settings) — Settings UI is the place to
  configure user-pack paths.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F16.
3. `/home/teague/code/BARMapEditor/crates/barme-core/src/water_presets.rs`
   — water preset pattern. Skybox/atmosphere presets follow the
   same model.
4. Vendor: identify CC0 skyboxes. AmbientCG has some; OpenGameArt
   has more. Audit licensing per source.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-43-skybox-library
```

## Step 3 — Scope

### 1. Vendor skybox assets

`scripts/fetch-skyboxes.sh` — SHA-pinned, idempotent script
mirroring `fetch-textures.sh` (Sprint 7 / D1).

Vendor ~8-12 CC0 skyboxes under `tools/skyboxes/<NN-name>/` with:
- `_px.png` / `_nx.png` / `_py.png` / `_ny.png` / `_pz.png` /
  `_nz.png` (6 faces, 1024² each).
- `thumbnail.png` (256² preview).
- `license.txt` (clearly cites the source + CC0).

Total disk: ~50 MB. Bundles with AppImage (Sprint 33).

### 2. Atmospheric preset patches

`crates/barme-core/src/atmosphere_presets.rs` (new):

```rust
pub enum AtmospherePreset {
    BrightDay,
    Sunset,
    Overcast,
    Night,
    AlienTwilight,
    DesertMidday,
    PolarDawn,
    Stormy,
}

pub fn apply_preset(preset: AtmospherePreset, project: &mut Project);
```

Each preset is a sparse-Option overlay onto the existing
mapinfo.atmosphere + lighting + skybox path. Anchored to real BAR
maps where possible (e.g., Coastlines = BrightDay).

### 3. Skybox picker UI

`crates/barme-app/src/ui/skybox_picker.rs` (new). Triggered from
the F9 form's Atmosphere tab's "Pick skybox…" button or from a
new `widgets::skybox_picker_grid` (mirror of `slot_picker_grid`).

Layout: 4-col grid of 96² thumbnails. Hover → preview face name +
license. Click → applies path to `Project.mapinfo.atmosphere.sky_box`.

### 4. Preset chip strip in F9 form

The F9 form's Atmosphere tab gains a chip strip at the top:
"Atmospheric preset: [BrightDay] [Sunset] [Overcast] [Night] ...".
Click any chip to apply its preset patch + skybox.

Per Sprint 19's tooltip convention, each chip's hover shows the
preset's mapinfo deltas ("Applies fog: blue, sun_dir: south,
skybox: clear-day").

### 5. Custom user skyboxes

The Settings UI (Sprint 32) gains a "User skyboxes directory"
path. The picker also scans this dir for `<name>/` folders matching
the same 6-face PNG layout. Falls back to the stock dir if not set.

### 6. Tests + rollup

- **Preset round-trip**: apply BrightDay → save → re-open →
  fields still match preset.
- **Skybox path resolution**: stock vs user-dir skybox paths
  resolve correctly.
- **Rollup**: STATUS UPDATEs (F16 done).

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on preset apply with deltas
listed; `trace!` on skybox file load.

## Step 5 — Out of scope

- **Procedurally-generated skyboxes** (atmospheric scattering
  sim) — Stage 3.
- **HDR skyboxes** — Stage 2 polish.
- **Skybox auto-rotate animation** — gameplay-side feature.

## Step 6 — Critical pitfalls

1. **License vetting**: every vendored skybox MUST be CC0 or
   equivalently permissive. Audit `license.txt` files.

2. **Cubemap face orientation**: standardise on GL convention
   per Sprint 28 / ADR-040 pitfall. Document in `tools/skyboxes/README.md`.

3. **Preset patches don't overwrite explicit user-set values**:
   apply a preset → preserve fields the user has manually edited
   (track via `Project.atmosphere_overrides` analogous to
   `water_overrides`).

4. **Disk size**: 8-12 skyboxes × ~5 MB each = ~50 MB. Acceptable
   for an AppImage; bigger and we'd need on-demand download.

## Step 7 — Exit criteria

- 4+ commits on `main`: vendor skyboxes, presets module, picker UI,
  preset chip strip + rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (F16 done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: click "Sunset" → atmosphere shifts to sunset hues
  + skybox loads + lighting updates. Click "BrightDay" → reverts.
- Final devlog: summary + "Sprint 44 = F17 pathability overlay" handoff.
