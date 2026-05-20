# Sprint 16 — Layered painter, Part 2: paint viewport + GPU composite + tiled COW masks (D9)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 16** — the middle of three sprints (15 / 16 / 17) that
rebuild texture painting around a Photoshop-style layered stack.
Sprint 15 shipped the data model + CPU bake to BMP; Sprint 17 ships
the Layers panel UI + DNTS hybrid emission. Sprint 16 owns the
interactive painting experience: a **top-down 2D paint viewport**,
the **GPU composite pipeline** that flattens N layers into a live
preview texture, **tiled COW masks** so 8192² × N memory stays
sane, and the **layer-mask brushes** (reveal / hide / smooth / fill).

After this sprint, the user can paint into a layer's mask and see
the composited result in real time in both the 2D paint viewport
and the existing 3D viewport. The Layers panel UI is still the
Sprint-9 inspector (Sprint 17 replaces it) — Sprint 16 ships a
minimal "active layer" selector strip so the user has SOMETHING to
direct paint into.

**Prerequisites:**
- Sprint 15 (D8) MUST be ticked. The layer stack data model + CPU
  bake are the contract this sprint hooks brushes + GPU composite
  into.
- Sprint 13 (renderer-depth rework, ADR-037) MUST be ticked.
  Sprint 16 introduces a second render target (the offscreen
  composite RT) and re-binds the terrain shader's diffuse texture
  at runtime — both rely on the depth + RT machinery Sprint 13
  fixed up.

**Out of scope:**
- The Photoshop-style Layers panel (Sprint 17).
- DNTS hybrid emission (Sprint 17) — Sprint 16's paint affects the
  diffuse composite only. Splat distribution + DNTS DDS continue to
  come from Sprint 12 / D6 unchanged.
- Per-layer transforms beyond "passthrough" — Sprint 16's GPU
  composite respects `LayerTransform` but the UI to edit it is
  Sprint 17.
- Custom texture import — Sprint 17.
- Sunsetting `inspector_splat` — Sprint 17. Sprint 16 leaves it in
  place; the new paint viewport is reached via a new
  `Tool::PaintLayer` variant (keyboard `L`).
- Undo for mask strokes — same deferral as Sprint 9's splat undo.
  The new tiled-COW masks make per-stroke undo feasible in
  principle, but the diff machinery is non-trivial; tracked as a
  Sprint-19+ follow-up. Mask edits are NOT undoable in Sprint 16.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.2 (diffuse dims),
   §2.1 #1 (texture pipeline memory — load-bearing for tiled COW),
   §2.1 #13 (GPU brush latency, 8 ms NFR — Sprint 16 paint strokes
   are masks not heightmaps but the same NFR applies).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §1
   (`64·N+1` is only the heightmap; masks are `512·N` per side —
   different math, easy to confuse), §4 (DXT1 compression — happens
   downstream of the bake, irrelevant to Sprint 16's live preview).
4. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`**
   §7.3 (engine composite math — Sprint 16 mirrors only the diffuse
   alpha-over part; per-fragment normal-mapping is in Sprint 17's
   hybrid path).
5. **ADRs:**
   - **ADR-038 (from Sprint 15)** — the layer schema you're now
     painting into.
   - **ADR-039 (NEW)**: GPU layered composite pipeline. Bind group
     layout, WGSL shader, dirty-tile invalidation rules, fallback
     when the GPU runs out of bindings (Sprint 16 caps at 16 layers
     for the GPU path; beyond that the CPU bake from ADR-038 is
     authoritative and the preview goes stale with a chip warning).
   - **ADR-040 (NEW)**: Top-down 2D paint viewport. Ortho camera,
     mouse → mask pixel mapping, brush ring overlay, mask preview
     mode toggle, viewport split with the 3D preview.
   - **ADR-018** (Brush trait, already shipped): mask brushes
     follow this pattern. Brush ids: `mask-reveal`, `mask-hide`,
     `mask-smooth`, `mask-fill`.
   - **ADR-033** (per-stroke COW undo) is NOT applied to masks in
     Sprint 16 (see Out-of-scope). The tiled-COW machinery from
     this sprint is a foundation a future sprint can hang undo off.
6. `/home/teague/code/BARMapEditor/crates/barme-core/src/layers.rs`
   (from Sprint 15) — the data model you'll extend with tiled-COW
   masks + brush traits.
7. `/home/teague/code/BARMapEditor/crates/barme-core/src/brushes/`
   (heightmap brushes — the architectural template). ADR-018 dirty-
   rect pattern.
8. `/home/teague/code/BARMapEditor/crates/barme-core/src/splat.rs`
   — Sprint 9's splat brushes. Layer mask brushes are similar shape
   but operate on grayscale (`Vec<u8>`) not RGBA.
9. `/home/teague/code/BARMapEditor/crates/barme-app/src/terrain.wgsl`
   — the existing fragment shader. Sprint 16 adds a new
   `composite.wgsl` for the offscreen layered composite; the
   terrain shader sources its diffuse FROM that composite (Sprint 16
   wires the texture binding swap).
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs`
    — bind group definitions. Sprint 16 adds a new render pipeline
    + offscreen render target for the composite.
11. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs`
    — `Tool` enum, `App` struct, central viewport pointer dispatch.
    The new `Tool::PaintLayer` variant slots in with keyboard `L`,
    `Icon::Brush`, label "Paint layer". (`Icon::Brush` already
    exists from Sprint 9.)
12. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md`
    — D9. Confirm ADR-039 + ADR-040 reservations match.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-layers-paint-viewport
./devlog/log.sh new stage-1-layers-gpu-composite
./devlog/log.sh new stage-1-layers-tiled-cow-masks
```

Three sub-items in this sprint (paint viewport, GPU composite, tiled
COW). One devlog folder each.

## Step 3 — Scope

Four commits on `main`:

### Commit 1 — Tiled COW masks + mask brushes

Replace Sprint 15's flat `Vec<u8>` mask with a tiled COW model so
8192² × N memory stays bounded. The public mask API stays the same
shape (`width`, `height`, `write_rect`, `sample`) — only the
storage flips.

- `crates/barme-core/src/layers/mask.rs` (move + extend Sprint 15's
  `LayerMask`):
  - `TileGrid<u8>` — `256 × 256` byte tiles in a flat `Vec<Tile>`
    indexed by (tile_x, tile_y). Each tile is either:
    - `Tile::Uniform(u8)` — the entire tile is one byte (common: a
      fresh-empty layer has all `Uniform(0)` tiles; a freshly-added
      base layer has all `Uniform(255)`).
    - `Tile::Pixels(Box<[u8; 256 * 256]>)` — concrete pixels, lazily
      allocated when a write touches a `Uniform` tile.
  - `LayerMask::write_rect(rect, |x, y| -> u8)` — dirty-rect write;
    promotes touched `Uniform` tiles to `Pixels` lazily.
  - `LayerMask::sample(x, y) -> u8` — fast path for `Uniform` tiles.
  - `LayerMask::dirty_tiles_since(snapshot_id) -> Vec<TileCoord>` —
    so the GPU composite can upload only the changed tiles.
  - Serde: `Tile::Uniform` is a 1-byte payload, `Pixels` is base64
    encoded under the same scheme as Sprint 15. Old projects load
    via a custom `Deserialize` that takes the Sprint-13 flat
    `bytes: Vec<u8>` and runs `TileGrid::from_bytes` to detect
    `Uniform` tiles. **Pin a regression test for this migration.**
- `crates/barme-core/src/layers/brushes.rs`:
  - `MaskBrush` trait, object-safe `Send + Sync + 'static`,
    same shape as Sprint 9's `SplatBrush`. Apply returns
    `Option<DirtyRect>`.
  - Four brushes, kebab ids matching what Sprint 17's UI dispatches
    on:
    - **`mask-reveal`** — push toward 255 with smoothstep falloff.
    - **`mask-hide`** — push toward 0 with smoothstep falloff.
    - **`mask-smooth`** — 3×3 mean blur; mirror Sprint 9's `Smooth`.
    - **`mask-fill`** — bucket fill from a click point. 4-connected
      flood, threshold = "all pixels within ±5 of the click point's
      current value get set to either 0 or 255 depending on a
      `target_visible: bool` arg." This is the only NON-falloff brush.
  - `MaskBrushRegistry::default_set()` ships those four.
  - Tests: each brush respects `DirtyRect`, idempotent at strength
    1.0, no-op at strength 0.0, no-op at radius 0.0, no-op when
    off-map.
- `crates/barme-core/src/layers.rs`:
  - `LayerStack::active_layer_mut(&mut self, id: &str) -> Option<&mut TextureLayer>`.
  - `LayerStack::apply_brush(&mut self, layer_id, brush, stamp)`:
    looks up the active layer, dispatches `brush.apply(&mut layer.mask, stamp)`,
    returns the `DirtyRect`. Sprint 16's central-viewport dispatch
    calls this.

### Commit 2 — GPU composite pipeline + offscreen target (ADR-039)

Build the live preview compositor. Goal: changing a single mask
tile triggers ≤8 ms re-composite.

- `crates/barme-app/src/composite.wgsl` (new):
  - One pipeline. Reads N layer slot diffuses (texture array,
    capacity 16 layers), N layer masks (`r8unorm`, also a texture
    array), and a uniform block with per-layer transforms +
    color + opacity + blend + active flag.
  - Per-pixel: iterate layers back-to-front (Sprint 16 = normal
    blend only). For each visible layer:
    1. Compute layer-local UV from the offscreen pixel coord via
       `LayerTransform` (mirror → rotate → scale → wallpaper-tile).
    2. Sample slot diffuse with `wallpaper-tile` (use `address_mode
       = repeat` on the sampler).
    3. Apply tint + brightness.
    4. Multiply RGB by `(mask × opacity)`.
    5. Alpha-over the accumulator.
  - Output: RGBA8 into the offscreen render target.
  - Layer cap: **16**. Above 16, the editor renders only the bottom
    16 and surfaces a "preview is approximate; build is exact" chip
    in the top bar (the CPU `bake_diffuse` from Sprint 15 is the
    authoritative render — the warning links to a tooltip
    explaining the discrepancy).
- `crates/barme-app/src/render.rs`:
  - Add `CompositeResources { rt: wgpu::Texture, rt_view: ..., bgl, pipeline, slot_diffuse_array, mask_array, uniform_buf }`.
  - `rt` is `rgba8unorm`, dims = `size.texture_dims()` clamped to
    `min(texture_dims, 4096²)` because storage textures over 4k²
    are not universally supported. **For maps with diffuse > 4096²,
    the composite runs at 4096² and is bilinearly upsampled when
    bound to the terrain shader; the final BMP export remains at
    full `texture_dims` via the CPU bake.** Note this clearly in
    ADR-039.
  - Slot diffuse array: 1024² × 16 RGBA8. Pre-loaded once on app
    start; the slot id → array layer mapping lives on
    `App::composite_layer_map` and updates when layers are
    added/removed.
  - Mask array: same dims as `rt` (so each layer's mask covers the
    full diffuse 1:1) × 16 `r8unorm` layers.
  - `write_layer_mask_tiles(layer_idx, dirty_tiles)`: per-tile
    sub-uploads matching ADR-017's heightmap pattern.
  - `recomposite_dirty(&mut Encoder, dirty_layers, dirty_tiles)`:
    runs the composite pass for affected pixels only (scissor +
    a viewport-spanning quad). For the typical "one brush stroke
    on one layer" case this is a sub-1ms operation.
- `crates/barme-app/src/terrain.wgsl`:
  - The DIFFUSE sample at `terrain.wgsl:148`-ish is currently the
    biome ramp + splat composite. Add a uniform flag
    `use_composite_rt: u32`; when set, sample the offscreen RT
    instead of the biome ramp. The splat composite (Sprint 9) stays
    in place — it OVERLAYS the composite RT for DNTS detail.
  - When `use_composite_rt = 0` (e.g. empty layer stack), fall back
    to the biome ramp.
- Tests:
  - `composite::tests::single_layer_full_mask_produces_slot_diffuse` —
    one base layer, full mask, RT sample at (512, 512) ≈ slot diffuse
    at the corresponding wallpaper-tile coord.
  - `composite::tests::two_layers_half_mask_alpha_over_correct` —
    base layer + top layer at 50 % mask, RT center ≈ 50/50 blend.
  - `composite::tests::transform_offset_wraps_wallpaper_seam` —
    layer with `offset_elmos = [4096.0, 0.0]` on an 8192-elmo map
    produces a half-shifted tile; no seam visible.

### Commit 3 — Top-down 2D paint viewport + active-layer strip (ADR-040)

The viewport switches modes based on `Tool`. When `Tool::PaintLayer`,
the central viewport renders the offscreen composite RT in 2D
ortho. Otherwise it stays in the existing 3D perspective.

- `crates/barme-app/src/ui/paint_view.rs` (new):
  - `pub fn paint_view(ui, ctx, app, rect)`:
    - Allocate a `Sense::click_and_drag` rect at the viewport size.
    - Pan: middle-click drag, or holding space + LMB. State on
      `App::paint_view_state { offset: Vec2, zoom: f32 }`. Default
      zoom = "fit map to viewport"; double-click resets.
    - Zoom: scroll wheel. Pivots on the cursor. Range 0.25× – 16×.
    - Render the composite RT scaled by `zoom`. Outside the map
      area (i.e. beyond the RT bounds), fill with `t.bg`.
    - Brush ring overlay: solid 1px circle in `accent` color at
      the cursor, with a smaller inner ring matching Sprint 9's
      style.
    - Mask-only preview toggle: `pill_toggle` in the top-right of
      the viewport that renders ONLY the active layer's mask as
      grayscale (red overlay where mask = 0). Useful when painting.
    - Status strip at the bottom: cursor's pixel coordinate + the
      active layer's mask value at that pixel.
- `crates/barme-app/src/main.rs`:
  - Add `Tool::PaintLayer` variant (keyboard `L`, icon
    `Icon::Brush`, label "Paint layer"). Mirror the Sprint 9
    pattern.
  - When `App::tool == Tool::PaintLayer`, the central viewport
    rendering switches: `if matches!(self.tool, Tool::PaintLayer) {
    paint_view::paint_view(...)} else {/* existing 3D */}`.
  - Pointer dispatch: LMB drag → for each stamp position, call
    `LayerStack::apply_brush(active_layer_id, brush, stamp)`,
    then enqueue the returned `DirtyRect` for the next frame's
    `recomposite_dirty` upload.
- **Minimal "active layer" selector** (Sprint 17 replaces with the
  full Layers panel):
  - In the right-side inspector, when `Tool::PaintLayer` is active,
    render a vertical strip of layer chips (one per
    `Project.layers.layers`, in top-of-stack-first order matching
    the Photoshop convention). Click selects active; the brush
    section below mirrors Sprint 9's BRUSH section (radius /
    strength / spacing sliders + four brush buttons —
    reveal / hide / smooth / fill).
  - This strip is NOT the final Layers panel — no drag-reorder,
    no thumbnails beyond a tiny slot swatch, no eye/lock toggles.
    It exists to make Sprint 16 self-sufficient for testing.
- **The 3D viewport continues to work in `Tool::PaintLayer`** via
  the existing minimap. The user can switch back to `Tool::Heightmap`
  / `Tool::SplatPaint` etc. to inspect the 3D preview; the
  composite RT stays bound to the terrain shader's diffuse so the
  3D view always reflects the latest paint.

### Commit 4 — Rollup

STATUS UPDATEs in SRS / ROADMAP, tick D9, write ADR-039 + ADR-040
(reserve in the ledger if not already done — the schema in Sprint 15's
ADR-038 stays; ADR-039 covers GPU; ADR-040 covers viewport), close
the three devlog folders.

## Step 4 — Standing constraints

Same as Sprint 15. `cargo fmt && cargo clippy --workspace
--all-targets -- -D warnings && cargo test --workspace` green
before every commit.

Tracing: `info!` on tool switch into `PaintLayer`; `debug!` on
per-stamp brush dispatch (rate-limited to ≤1/frame to avoid log
flood during drag); `trace!` on per-tile GPU uploads.

## Step 5 — Out of scope (loud)

- **No Layers panel.** Sprint 17. Sprint 16's "active layer strip"
  is intentionally minimal.
- **No custom texture import.** Sprint 17.
- **No DNTS hybrid emission.** Sprint 17. Sprint 16's bakes go
  through Sprint 15's diffuse path only.
- **No undo for mask strokes.** Tracked as a future sprint; the
  COW machinery from Commit 1 makes it feasible but the
  per-stroke diff format is non-trivial.
- **No transform editing UI.** `LayerTransform` is read but not
  written from any Sprint 16 UI. Default = identity. Sprint 17's
  Layers panel adds the controls.
- **No blend mode selector.** `BlendMode::Normal` is the only
  variant; the composite shader hard-codes alpha-over.
- **No imported-texture rendering.** Sprint 16's slot diffuse
  array only sources from the stock `tools/textures/<NN-slot>/`
  registry. `LayerSource::Imported` layers fall back to a magenta
  diagnostic texture with a warn-tone chip surfaced in the
  paint view ("Imported layers preview disabled until Sprint 17.").

## Step 6 — Critical pitfalls (read twice)

1. **Mask byte-size budget.** Tiled COW with 256² tiles means a
   freshly-created layer's mask is N × `Tile::Uniform(0)` —
   under 1 KB regardless of map size. Once the user paints, only
   touched tiles allocate to `Pixels`. A typical 16-SMU map has
   `8192 / 256 = 32` tiles per side = 1024 tiles per layer; a
   typical stroke touches ~5-20 tiles. Memory scales with paint
   coverage, not map size.
2. **Wallpaper sampling on the GPU.** The composite shader's
   sampler MUST be `address_mode_u/v/w = Repeat` so out-of-bounds
   UVs wrap. If you use `ClampToEdge` (wgpu's default), scaled-down
   textures stretch instead of tile and you'll get the "smeared
   seam" look the user explicitly rejected.
3. **Brush dispatch must produce ALL stamps along a drag.** Mirror
   Sprint 9's pattern: when a drag delta exceeds `spacing × radius`,
   interpolate intermediate stamps. Without this, fast drags leave
   gaps.
4. **GPU bind budget.** 16-layer texture arrays + 16-layer mask
   arrays + a uniform block = 3 bindings. Add slot diffuse sampler,
   mask sampler, output write = ~6 bindings. Well under wgpu's
   16-binding default.
5. **Composite RT max dim is 4096².** Maps larger than 8 SMU
   diffuse > 4096²; the RT runs at 4096² and is bilinearly upscaled
   when bound to the terrain shader. The CPU bake (Sprint 15)
   produces the full-res image for `.sd7` export. Surface the
   discrepancy with a chip ("Preview at half-res — build is full").
6. **Mask sub-uploads, not full-texture.** A full mask write at
   8192² × 16 layers = 1 GB upload per frame. ALWAYS upload only
   the dirty tiles returned by the brush's `DirtyRect`. Pin this
   with a `debug_assert!` in `write_layer_mask_tiles`.
7. **Active layer selection persists across tool switches.** The
   user expects clicking back to `Tool::PaintLayer` to resume on
   the same active layer they left. State lives on
   `App::paint_active_layer_id: Option<String>` and is restored on
   tool re-entry.
8. **Paint view ortho aspect.** The 2D viewport's "pixel aspect" must
   be 1:1 even when the viewport rect's aspect doesn't match the
   map's. Letterbox the empty bands; do NOT stretch the composite
   RT to fill non-square viewports.
9. **The 3D viewport's diffuse texture binding changes once the
   composite RT exists.** Old `synth_grey_bmp` / biome-ramp paths
   in the WGSL stay as fallbacks but the live preview should always
   route through the composite RT when `Project.layers.layers > 0`.
   This is a render-state change in Sprint 16, NOT a Sprint 17
   item.
10. **`Tool::PaintLayer` is independent of `Tool::SplatPaint`.**
    Sprint 9's splat painting still works — it paints into the
    legacy `splat_distribution` which drives runtime DNTS. The two
    tools coexist for one more sprint; Sprint 17 retires
    `Tool::SplatPaint`.
11. **Off-map paint clipping.** When the cursor leaves the map
    rect, the brush stamp clips silently. Do NOT crash on negative
    pixel coords from the brush math.

## Step 7 — Exit criteria

- 4 commits on `main`: tiled COW + mask brushes, GPU composite,
  paint viewport + active-layer strip, rollup.
- 3 devlog folders filled.
- 1 checkbox ticked in `phase-3-plan.md` (D9).
- ADR-039 + ADR-040 in `docs/DECISIONS.md`.
- SRS / ROADMAP STATUS UPDATEs (paint viewport + GPU composite +
  tiled COW masks shipped, Layers panel + custom texture import +
  DNTS hybrid gated on Sprint 17).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
  && cargo test --workspace` green.
- Smoke test (record in final devlog log):
  - Launch the editor, hit `L` → tool strip highlights "Paint
    layer", central viewport flips to top-down 2D, the inspector
    shows the active-layer strip + brush controls.
  - Pan + zoom in the 2D viewport with middle-drag + scroll wheel.
  - Select a non-base layer (Sprint 15's migration may have
    seeded 4 layers from `splat_config`; otherwise add a placeholder
    layer with a TEST harness — Sprint 17 will be the user-facing
    add).
  - Paint a stroke with `mask-reveal` → the painted region of the
    selected layer becomes visible IN THE COMPOSITE; switch to
    `Tool::Heightmap` (`B`) → the 3D viewport's terrain shows the
    same composite (bound via the offscreen RT).
  - Toggle "mask-only preview" → the 2D viewport renders just the
    active layer's mask as grayscale.
  - Paint a fast drag (mouse moves 500 px in one frame) → no gaps
    in the resulting stroke; the spacing interpolation kicks in.
  - Switch back to `Tool::PaintLayer` after touching another tool
    → the previously-active layer is still selected.
  - Save the project, reopen → masks round-trip; the painted region
    is still painted.
  - Build a `.sd7` → the diffuse BMP reflects the painted layers
    (Sprint 15's CPU bake reads the same masks). Load in BAR →
    the painted layer is visible in-game.
  - `cargo test --workspace -- composite layers brushes` runs all
    new tests green.
- Final devlog log summarizes what shipped + "Sprint 17 = Layers
  panel UI + DNTS hybrid emission + custom texture import +
  retire `inspector_splat` (D10)" handoff note.

Start by running `git status`, then read
`crates/barme-core/src/splat.rs` end-to-end — its `SplatBrush`
trait is the architectural template for `MaskBrush` and getting
the dirty-rect contract right up front saves you a round of
rework later. Begin with Commit 1 (tiled COW masks + brushes) —
Commits 2 and 3 depend on its dirty-rect API.
