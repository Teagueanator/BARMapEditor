# Sprint 13 — Renderer foundation: depth + GPU markers (ADR-037)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 13** — the FIRST sprint of the renderer-parity arc
(Sprint 13 + Sprints 20–27 planned, see `docs/research/renderer-bar-parity/ROADMAP.md`).
The arc was originally drafted as Sprints 15–23; the painter (15–17)
and water/lava (14) sprints were inserted ahead, pushing the
remaining renderer-parity sub-sprints back by 4. The numbering below
reflects the NEW positions.
You move terrain and 3D-positioned markers from a flat
"2D-painter-on-top-of-flat-wgpu-pass" architecture into a real
**offscreen render target with a depth attachment**, then composite the
result into egui as a texture. After this sprint, markers depth-test
against terrain (a start-position behind a hill is occluded by the
hill), and translucent markers blend in correct camera-relative order
when the user orbits.

**Renderer-arc context.** The 2026-05-18 user direction reversed
SRS §2.1 #11 ("3D preview ≠ in-game rendering. Document the gap up
front; do not pretend WYSIWYG"). The new policy is: **the editor must
visually reproduce what Recoil renders for every BAR map feature** —
terrain (DNTS + lighting + spec + normals), atmosphere (fog + sun +
sky), water (surface + reflections + foam + caustics), shadows,
features (S3O / 3DO models), grass, emission (lava glow), skybox
reflections. That's a multi-sprint arc; Sprint 13 is the foundation
that makes the rest possible. SRS § 2.1 #11 has a STATUS UPDATE
documenting the policy reversal.

This is **advanced work**. It is one focused sprint but a large one — touches
the wgpu pipeline, the shader, the central-viewport paint flow, and the
marker / overlay surfaces in `ui/overlay.rs` + `main.rs::central`.

**Motivation:** the renderer audit in
`devlog/stage-1-sprint-prompts-audit/notes.md` (2026-05-18) found that all
3D-positioned UI elements (start positions, brush rings, mirror ghosts,
symmetry axes) are CPU-projected via `render::world_to_screen` and painted
as flat 2D shapes on top of an opaque terrain pass that has **no depth
attachment** (`crates/barme-app/src/render.rs:233`,
`depth_stencil: None`). Symptoms:

- Translucent markers blend in iteration order, NOT depth order. Orbit the
  camera 180° and a "back" marker still draws over a "front" one.
- Markers cannot be occluded by terrain — a start position behind a
  mountain still shows through.
- Sprint 11 (metal + geo, already shipped), 12 (features + splat
  emission), 14 (water plane), 15-17 (painter), 18 (minimap) all add
  more 3D-positioned markers / passes that inherit (or, post-Sprint-13,
  benefit from the fix to) this bug.

**Prerequisites:** Sprints 1–12 done. The original plan recommended
running this BEFORE Sprint 11 to avoid retrofit work; that ship has
sailed — Sprints 11 / 12 shipped with the old 2D-painter pattern, so
this sprint retroactively ports their markers (metal spots, geo vents,
feature placement) as part of Phase 4 below. The diff is larger than
the original plan anticipated; treat the retrofit as in-scope work.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §2.1 #11 ("3D preview ≠
   in-game rendering" — sets the bar: the editor preview is an
   approximation, NOT a full PBR + atmosphere reproduction), §3.3
   NFR-Performance (8 ms brush stroke budget — depth-write pass adds
   load).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §1 (texture
   pipeline memory budget — the offscreen RT adds 16 MB / Mpixel at
   RGBA8 + Depth32; cap on iGPU).
4. **`/home/teague/code/BARMapEditor/devlog/stage-1-sprint-prompts-audit/notes.md`**
   — the audit that motivated this sprint. Reread the "Findings" table
   carefully.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/render.rs` —
   current renderer. Pay attention to:
   - `Uniforms` struct (line 23).
   - `RenderResources` (line 44) — gains: offscreen color/depth targets,
     marker pipeline + instance buffer.
   - `install` (line 116) — gains: offscreen RT alloc + marker pipeline
     setup.
   - `upload_heightmap` (line 267) — gains: depth-target resize.
   - `OrbitCamera::framing` (line 363) — `near: 10, far: max * 8`. We'll
     auto-tune near.
   - `world_to_screen` (line 425) — survives, but no longer the
     primary marker positioning path. Symmetry axes still use it.
   - `TerrainCallback` (line 507) — the `prepare()` hook is where we
     encode the offscreen render pass.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/terrain.wgsl` —
   current shader. Survives; pipeline gets a depth attachment, fragment
   stage unchanged.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/overlay.rs` —
   current 2D marker / axis painter helpers. After this sprint, ghost
   rings + primary brush ring become MarkerBatch entries; symmetry
   axes remain in egui::Painter (Step 3 details).
8. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs::central`
   (line 4382) — the paint flow. Big restructuring.
9. ADR-017 (heightmap GPU upload pattern — extends with depth + marker
   buffers).
10. ADR-035 (UI overhaul — clarifies that the viewport chrome is
    intentionally 2D screen UI; rulers, minimap, hint card stay outside
    the offscreen render).

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-renderer-depth-rework
./devlog/log.sh log stage-1-renderer-depth-rework "starting"
```

Fill `stage-1-renderer-depth-rework/`:
- `goals.md` — from the success criteria below.
- `theories.md` — the hypothesis that offscreen-RT + Callback.prepare
  encoding cleanly slots in without forking eframe.
- `notes.md` — design sketches, wgpu API choices, perf measurements.
- `logs/<timestamp>__<title>.md` — session logs.

## Step 3 — Scope

In order, one commit per phase. **6 commits + rollup.**

### Phase 1 — Offscreen render target

`crates/barme-app/src/render.rs`:

```rust
struct OffscreenTarget {
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    /// egui_wgpu::Renderer's texture id so `ui.painter().image()` can
    /// reference this offscreen color buffer.
    egui_texture_id: egui::TextureId,
    size: (u32, u32),
}
```

- Color format: `wgpu::TextureFormat::Rgba8UnormSrgb` (matches typical
  swapchain). Set `usage = TEXTURE_BINDING | RENDER_ATTACHMENT |
  COPY_SRC` so we can both render INTO it and SAMPLE it from egui.
- Depth format: `wgpu::TextureFormat::Depth32Float`. `usage =
  RENDER_ATTACHMENT` only (we don't sample depth this sprint;
  Sprint 16 / D7 minimap could).
- Size: physical pixels of the central viewport rect. Compute from
  `egui::Rect.size() * pixels_per_point`.
- `egui_texture_id`: register the color view with
  `egui_wgpu::Renderer::register_native_texture(...)`. Cache the id; if
  the texture gets resized, re-register and store the new id.

**Resize logic:**

The central rect size changes when the user resizes the window. Track
`self.offscreen.size` and reallocate when it differs from the current
rect's physical size. Reallocation cost: ~1-2 ms; acceptable on resize
edges. Don't reallocate every frame — gate on size diff.

**Caveat for iGPU:** at 4K display × pixels_per_point=2 = 8K pixels
across. Offscreen RT = 4 + 4 = 8 bytes / pixel × 8K × 4K = 256 MB. Cap
the offscreen size to `min(rect_physical, 2048²)` per axis and let egui
upscale the image widget. Document the cap.

Touch points: `crates/barme-app/src/render.rs`. Sub-commit ends when
the offscreen RT allocates + resizes cleanly (no rendering yet).

### Phase 2 — Migrate terrain pipeline to offscreen + depth

`crates/barme-app/src/render.rs`:

- Update `install()`'s pipeline creation:
  ```rust
  depth_stencil: Some(wgpu::DepthStencilState {
      format: wgpu::TextureFormat::Depth32Float,
      depth_write_enabled: true,
      depth_compare: wgpu::CompareFunction::Less,
      stencil: wgpu::StencilState::default(),
      bias: wgpu::DepthBiasState::default(),
  }),
  ```
- Color target format becomes `Rgba8UnormSrgb` (offscreen color
  format), NOT `render_state.target_format` (which is the swapchain).
- `TerrainCallback::prepare()`: encode a render pass into the offscreen
  target. Use the encoder argument:
  ```rust
  let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
      label: Some("offscreen.terrain"),
      color_attachments: &[Some(wgpu::RenderPassColorAttachment {
          view: &res.offscreen.color_view,
          resolve_target: None,
          ops: wgpu::Operations {
              load: wgpu::LoadOp::Clear(wgpu::Color {
                  r: 0.04, g: 0.05, b: 0.07, a: 1.0,
              }),
              store: wgpu::StoreOp::Store,
          },
      })],
      depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
          view: &res.offscreen.depth_view,
          depth_ops: Some(wgpu::Operations {
              load: wgpu::LoadOp::Clear(1.0),
              store: wgpu::StoreOp::Store,
          }),
          stencil_ops: None,
      }),
      timestamp_writes: None,
      occlusion_query_set: None,
  });
  // ... set pipeline, bind group, index buf, draw_indexed ...
  ```
- The Callback's `paint()` (which runs inside egui-wgpu's own pass)
  becomes a NO-OP — we've already rendered to offscreen in `prepare`.
  Return early.
- `central()` paints the offscreen color texture into the viewport rect:
  ```rust
  ui.painter().image(
      offscreen.egui_texture_id,
      rect,
      egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
      egui::Color32::WHITE,
  );
  ```
  Replaces the existing `Callback::new_paint_callback` placement —
  the Callback STILL fires (to encode the offscreen pass in
  `prepare`), but no longer renders into egui's color attachment.

**Verification:** at the end of Phase 2, the viewport renders the
same image as before (terrain on a dark background) but via the
offscreen path. Markers / overlays not yet ported — they still paint
on top via egui::Painter, same as today. The bug is unchanged at this
point; we're just changing the plumbing. **Cargo test green.**

### Phase 3 — Camera near-plane auto-tune + standardize premultiplied alpha

Two small QoL fixes that ride with the renderer rework:

**`OrbitCamera::view_proj_matrix`** (`render.rs:383`):

The `near` field is no longer purely cosmetic — depth precision now
matters. Tune:

```rust
pub fn near_far(&self) -> (f32, f32) {
    // Conservative: place near at 1% of distance, far at 4× the
    // diagonal-distance-to-target so even tilted-down views see the
    // far edge of large maps. Clamp near to a sane floor so users
    // zooming way out don't blow up the precision.
    let near = (self.distance * 0.01).max(50.0);
    let far  = (self.distance * 4.0).max(near * 100.0);
    (near, far)
}
```

Then `view_proj_matrix` reads `let (near, far) = self.near_far();`
instead of `self.near` / `self.far`. The struct fields stay
(other code reads them; deprecate gradually).

**Standardize on premultiplied alpha** — `Color32::from_rgba_unmultiplied`
call sites in `main.rs::central` should be migrated to
`from_rgba_premultiplied` once Phase 4's MarkerBatch is in place
(MarkerBatch takes premultiplied colors directly). The 2D start-pos
labels can stay unmultiplied for now (egui's text path is consistent
across both).

Test: `cargo test -p barme-app -- camera near_far` — pin the formula.

### Phase 4 — GPU marker pipeline + `MarkerBatch`

This is the heart of the sprint. A second wgpu pipeline that renders
billboarded markers with proper depth-testing.

**New shader** `crates/barme-app/src/markers.wgsl`:

```wgsl
struct MarkerU {
    view_proj: mat4x4<f32>,
    viewport_size: vec2<f32>,  // px
    _pad: vec2<f32>,
};

struct Instance {
    world_pos: vec3<f32>,
    radius_px: f32,
    color: vec4<f32>,          // premultiplied
    shape_id: u32,             // 0=filled-circle, 1=outline-ring,
                               //   2=filled-with-stroke
    _pad: vec3<u32>,
};

@group(0) @binding(0) var<uniform> u: MarkerU;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,           // [-1, 1]² across the quad
    @location(1) @interpolate(flat) inst: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32,
           @builtin(instance_index) iid: u32) -> VsOut {
    let inst = instances[iid];

    // Project marker centre to clip space.
    let centre_clip = u.view_proj * vec4<f32>(inst.world_pos, 1.0);

    // Unit quad vertices: (-1,-1), (1,-1), (-1,1), (1,1). 4 verts per
    // instance, drawn as TriangleStrip.
    let corner = vec2<f32>(
        f32((vid & 1u) * 2u) - 1.0,
        f32(((vid >> 1u) & 1u) * 2u) - 1.0,
    );

    // Offset in clip space by the screen-space radius.
    let px_to_clip = inst.radius_px * 2.0 / u.viewport_size;
    let offset_clip = corner * px_to_clip * centre_clip.w;

    var out: VsOut;
    out.clip_pos = centre_clip + vec4<f32>(offset_clip, 0.0, 0.0);
    out.uv = corner;
    out.inst = iid;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let inst = instances[in.inst];
    let d = length(in.uv);
    switch inst.shape_id {
        case 0u: {  // filled circle
            if (d > 1.0) { discard; }
            // soft 1-px AA edge
            let a = 1.0 - smoothstep(1.0 - 0.05, 1.0, d);
            return inst.color * a;
        }
        case 1u: {  // outline ring (thickness ~ 0.1 of radius)
            if (d > 1.0 || d < 0.85) { discard; }
            let a = (1.0 - smoothstep(0.95, 1.0, d))
                  * smoothstep(0.85, 0.90, d);
            return inst.color * a;
        }
        default: {  // filled with white stroke (start-pos source)
            if (d > 1.0) { discard; }
            // Inner fill: inst.color; outer ring: white border at 0.85..1.0.
            if (d > 0.85) {
                return vec4<f32>(1.0, 1.0, 1.0, inst.color.a);
            }
            return inst.color;
        }
    }
}
```

**Marker pipeline state:**

```rust
depth_stencil: Some(wgpu::DepthStencilState {
    format: wgpu::TextureFormat::Depth32Float,
    depth_write_enabled: false,       // ← markers DON'T write depth
    depth_compare: wgpu::CompareFunction::Less,  // ← but DO test it
    stencil: wgpu::StencilState::default(),
    bias: wgpu::DepthBiasState::default(),
}),
blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
write_mask: wgpu::ColorWrites::ALL,
primitive: wgpu::PrimitiveState {
    topology: wgpu::PrimitiveTopology::TriangleStrip,
    ..
},
```

Critical: `depth_write_enabled: false` + `depth_compare: Less`. This is
the standard transparency rule — markers occlude against terrain (which
DID write depth) but don't occlude each other via depth. They blend
back-to-front via alpha, sorted CPU-side.

**`MarkerBatch` accumulator** in `crates/barme-app/src/ui/markers.rs` (new):

```rust
pub struct Marker {
    pub world_pos: glam::Vec3,
    pub radius_px: f32,
    pub color: [u8; 4],   // premultiplied
    pub shape: MarkerShape,
}

pub enum MarkerShape { FilledCircle, OutlineRing, FilledWithStroke }

#[derive(Default)]
pub struct MarkerBatch {
    items: Vec<Marker>,
}

impl MarkerBatch {
    pub fn push(&mut self, m: Marker) { self.items.push(m); }
    pub fn extend(&mut self, m: impl IntoIterator<Item = Marker>) {
        self.items.extend(m);
    }
    /// Sort by view-space Z (largest-Z first = farthest first =
    /// back-to-front). Called by `render.rs` before encoding the
    /// instance buffer.
    pub fn sort_back_to_front(&mut self, view: glam::Mat4) {
        self.items.sort_by(|a, b| {
            let za = (view * a.world_pos.extend(1.0)).z;
            let zb = (view * b.world_pos.extend(1.0)).z;
            // For LH coords, smaller Z = closer to camera. Sort
            // DESCENDING Z = farthest first.
            zb.partial_cmp(&za).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    pub fn into_instances(self) -> Vec<MarkerInstanceGpu> { ... }
}
```

**Per-frame flow** (in `TerrainCallback::prepare`):

1. The App pushes markers into `MarkerBatch` during `central()`
   before adding the Callback. Pass the batch into the Callback.
2. `Callback::prepare` sorts the batch back-to-front using the view
   matrix.
3. Convert to instance buffer; upload via `queue.write_buffer`.
4. Begin offscreen render pass (terrain pipeline, then markers
   pipeline).
5. Terrain draws first (with depth-write); markers draw second (with
   depth-test, no depth-write).

**Marker accumulation in `central()`:** replace the existing
`painter.circle_filled / circle_stroke` calls in `main.rs:4621-4701`
with `batch.push(Marker { ... })`. The marker pipeline draws them all
in one indirect call.

The brush ghost rings in `paint_brush_ghosts` (overlay.rs) similarly
move into the batch. The primary brush ring stays in the batch too
(for consistency).

**`world_to_screen` survives** for: symmetry axes (Step 5), start-pos
text labels (still drawn via egui::Painter on top of the composited
image), and the cursor-projection helper. Don't delete it.

### Phase 5 — Symmetry axes via wgpu line pass

`overlay.rs::paint_symmetry_overlay` was painting dashed lines via
egui::Painter, and skipping the entire axis if either endpoint clipped
off-screen (audit finding #3). With a depth buffer in play, axes
should:

- Render in 3D so they're depth-occluded by hills.
- NOT skip on edge-clipping; the wgpu line clipper handles that.

**Implementation:**

Add a third pipeline in `render.rs`: line-list topology, vertex shader
generates clip-space positions from world-space endpoints, fragment
shader writes the axis color. Build a small dashed stipple either:
- Procedurally in the fragment shader using `gl_FragCoord` mod stride,
  OR
- By generating multiple short segments CPU-side and emitting them as
  LineList instances (cheaper to debug; matches the existing
  `dash_subsegments` logic).

Recommend the second approach — reuses `dash_subsegments` from
`overlay.rs`, just emits world-space endpoints instead of screen-space.

`paint_symmetry_overlay` becomes
`overlay::collect_symmetry_segments(symmetry, extents, batch)` —
returns world-space line segments that the wgpu line pipeline draws
with depth test.

Line pipeline depth state:
```rust
depth_write_enabled: false,
depth_compare: wgpu::CompareFunction::Less,
```

Same as markers — line segments occlude against terrain (a horizon
mountain occludes the axis where it crosses the mountain) but don't
write depth themselves.

### Phase 6 — Migrate start-position labels + minimap overlay

Text labels (the per-position index numbers) STAY in egui::Painter on
top of the composited image. Reasons:
- SDF / atlas text rendering in wgpu is a non-trivial sub-project.
- Labels are annotation; arguably correct to always-show even when the
  marker is occluded.
- The label can use the marker's projected screen position from a
  new helper `markers::project_to_screen(&MarkerBatch, view_proj,
  viewport_size, rect)` that mirrors the GPU shader's projection math
  exactly — keeping CPU and GPU paths in sync.

`world_to_screen` gets a refinement: don't return None on
`|ndc.x| > 1 || |ndc.y| > 1`. Return the projected position
regardless; let the caller decide to clip via egui's painter rect
clip (`ui.painter_at(rect)`). Only return None on `clip.w <= 0`
(behind the camera).

Mini-map (`ui/minimap.rs`) is unchanged — it's a 2D top-down widget
that doesn't touch the 3D pipeline.

Rulers, hint card, viewport-options toolbar all stay as 2D UI on top
of the composited image. They're screen-aligned, not 3D.

### Phase 7 — Rollup

- New `ADR-037` in `docs/DECISIONS.md`:
  - Title: "Offscreen render target + GPU markers pipeline"
  - Context: the audit finding + sprint motivation.
  - Decision: offscreen RT pattern via `egui_wgpu::Callback::prepare`,
    separate marker pipeline with depth-test-only.
  - Consequences: terrain occlusion works; translucent markers
    blend correctly; ~16 MB / Mpix offscreen memory budget on iGPU.
  - Alternatives considered: (a) sort markers in egui::Painter
    (rejected — doesn't solve terrain occlusion or future GPU
    work), (b) fork egui-wgpu to provision a depth target (rejected
    — fragile, ties us to a specific eframe version).
- STATUS UPDATEs in SRS / ROADMAP.
- phase-3-plan.md: add a "Stream X — Renderer rework" section under
  Stream B (UX overhaul) with the new ADR-037 reservation; tick the
  checkbox when this sprint closes.
- Final devlog log:
  - Note any wgpu API quirks encountered (eframe version
    compatibility, validation-layer warnings).
  - Capture timing measurements: terrain pass ms, marker pass ms,
    composite ms.
  - "Sprint 16 = TBD" handoff. Likely a polish item or the C5/D6
    integration if not done yet.

## Step 4 — Standing constraints

- `source ~/.cargo/env`.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- No `Co-Authored-By: Claude`.
- Terse commit subjects.
- Local-only.
- SRS source of truth.
- Tracing: `info!` on offscreen RT alloc / resize; `trace!` on marker
  count per frame; `warn!` on offscreen size clamped.
- Devlog folder (one for the whole sprint — 6 phases are tightly
  coupled).

## Step 5 — Out of scope (deferred to subsequent renderer-arc sprints)

The renderer-parity arc breaks the work into focused sprints. This
sprint ships ONLY the foundation. Each item below has a successor:

- **Terrain shader parity (DNTS, base normal R+A encoding, lighting,
  specular exponent, SMF_INTENSITY_MULT)** — **Sprint 20**. Subsumes
  the splat-shader work from Sprint 9 / D4 once the foundation is in
  place; integrates with the Sprint 17 layered composite RT (the
  diffuse source under the DNTS layer).
- **Atmosphere (fog start/end/color, sun direction, sky color/box,
  exponential height fog)** — **Sprint 21**.
- **Water polish (planeColor / surfaceColor / fresnel, planar
  reflections, refractions, foam at shorelines, caustics, perlin wave
  surface)** — **Sprint 22**. **The MVP flat alpha-blended water plane
  ships in Sprint 14 / C9**; this sub-sprint adds the visual polish on
  top of the already-rendering plane.
- **Directional shadows (cascaded shadow map sampled in fragment
  shader, groundShadowDensity controls intensity)** — **Sprint 23**.
- **Feature rendering (load S3O / 3DO models, instanced draw with
  diffuse / normal / specular textures + team-color)** — **Sprint 24**.
- **Grass (instanced quads with wind animation, mapinfo.grass.* tuning)**
  — **Sprint 25**.
- **Emission (lava glow from lightEmissionTex) + skybox cubemap
  reflections (skyReflectModTex) + parallax (if engine consumes it)**
  — **Sprint 26**. Pairs naturally with the Lava / Magma presets
  shipped in Sprint 14 / C9 — the emission texture is what makes a
  lava map actually glow at night.
- **Parity validation against actual BAR screenshots + drift list +
  final SRS §2.1 #11 retirement** — **Sprint 27**.

Explicitly NOT in any of the above:

- **MSAA on the offscreen target.** Add at any time once Phase 1 is
  stable; needs a resolve attachment + `sample_count > 1` on every
  pipeline. Not blocking on parity.
- **HDR rendering.** Color format stays `Rgba8UnormSrgb` through the
  arc. HDR (Rgba16Float + tone-mapping) is a Stage 2 ask.
- **GPU instance culling.** CPU sort is fine for <10k markers; revisit
  if Sprint 20's S3O feature counts push past 10k.
- **GPU-accelerated brush stamps.** Already ADR-021 deferred to Stage 2.
  This sprint doesn't touch the heightmap edit path.
- **Text rendering in wgpu.** Labels stay in egui::Painter throughout
  the arc.

## Step 6 — Critical pitfalls (read twice)

1. **`egui_wgpu::Callback::prepare`'s encoder is per-frame and shared
   with egui's own encoding.** Don't `finish()` it yourself; just
   begin/end render passes on it. The signature returns
   `Vec<CommandBuffer>` — return empty if your work is in the shared
   encoder. Return a separate CommandBuffer ONLY if you allocated a
   separate encoder via `device.create_command_encoder` (which we don't
   need for this sprint).

2. **Texture format mismatch is silent.** The terrain pipeline must
   declare the color target as `Rgba8UnormSrgb`, matching the
   offscreen color texture's format. If the formats differ, wgpu
   validation rejects the pipeline at pipeline creation time; the
   error message is clear. If you instead matched
   `render_state.target_format` (the swapchain format), the pipeline
   would silently render to the wrong target. **The pipeline's color
   target format MUST match the actual RenderPassColorAttachment's
   view format.**

3. **Depth attachment requires `RENDER_ATTACHMENT` usage flag** on the
   depth texture (not just `TEXTURE_BINDING`). Easy to miss; wgpu's
   validation will reject it.

4. **Depth32Float vs Depth24PlusStencil8.** We don't need stencil. Use
   `Depth32Float` — better precision, no stencil overhead, supported
   on all modern desktop GPUs. Check the adapter's feature set;
   downgrade to `Depth32FloatStencil8` if `Depth32Float` is missing
   (rare on desktop; might matter for Linux ARM later).

5. **`egui_wgpu::Renderer::register_native_texture` returns a
   `TextureId`. Keep it alive.** Each call increments an internal
   counter. Re-register on resize, but ONLY when the size actually
   changes (compare `(width, height)` tuple). Re-registering every
   frame leaks GPU texture handles.

6. **Premultiplied alpha is non-negotiable.** `BlendState::PREMULTIPLIED_ALPHA_BLENDING`
   in the marker pipeline AND the marker shader's output color must be
   premultiplied (`rgb *= a` before output). Mixing premul and
   straight alpha across the pipeline produces "white halo on
   transparent edges" — the classic giveaway.

7. **Back-to-front sort is approximate.** Two markers at the same
   view-space Z with different XY positions can interleave wrong if
   they're both translucent AND overlap. Acceptable for editor markers
   (which are small disks). For Sprint 12's F7 features (larger meshes)
   we may need to revisit, but defer.

8. **The depth buffer cleared to 1.0** (the far plane). Terrain writes
   depth values < 1.0; markers test `Less` so they pass against the
   far-plane background but not against terrain pixels in front of
   them. Verify: paint a start-position behind a hill, orbit. The hill
   should occlude the marker.

9. **NDC depth range is `[0, 1]` for wgpu**, NOT `[-1, 1]` like
   OpenGL. `Mat4::perspective_lh` from glam produces the wgpu-correct
   range by default (it's the wgpu/Vulkan/D3D convention, not the
   OpenGL one). If you see "everything z-fights at near plane,"
   check that you're not using `perspective_lh_zo` vs `perspective_lh`
   inconsistencies. glam's `perspective_lh` is the right call.

10. **Markers at world Y=0 with terrain at world Y=0 z-fight.** A
    start position has `y = 0.0`. If the terrain heightmap at
    that pixel is also 0, the marker's clip-space Z equals the
    terrain's clip-space Z, and `Less` rejects it (the marker
    disappears). Fix: lift markers by a small world-space epsilon
    (`y = 2.0` elmos) at marker construction. Alternative: enable
    a small `bias.constant: 5` in the marker pipeline's depth bias.
    Pick one; document.

11. **The mini-map's `paint_minimap` uses
    `ui.painter()` directly** (not the rect-clipped painter). It
    draws OUTSIDE the offscreen image. Don't touch it.

12. **Resize timing.** When the user resizes the window, the central
    rect changes, and on the SAME frame the offscreen RT may not have
    been resized yet (egui paints during update; the resize happens at
    the start of the next frame). Detect mismatched dims in
    `Callback::prepare` and skip the offscreen pass for one frame —
    the previous frame's image stays on screen for ~16 ms during
    resize. Less jarring than a green flash from an uncleared RT.

13. **Validation layer is your friend.** Run with
    `RUST_LOG=wgpu_core=warn,wgpu_hal=warn cargo run -p barme-app` and
    watch for "format mismatch" / "missing usage" warnings. Don't
    suppress them in the boot-log filter (A2 / `RUNTIME-WARNINGS.md`)
    until you've cleared them.

14. **Don't drop the brush ring's interactivity.** When the user moves
    the cursor, the brush ring follows. Migrating it to the marker
    batch means re-pushing the cursor marker every frame — that's
    fine for `<10k markers` but make sure the marker batch is cleared
    at the START of each frame, not lazily.

15. **Performance budget.** The audit found no current perf
    regression, but the offscreen RT + extra pass adds ~1–2 ms.
    Should fit within the 16 ms frame budget at 16 SMU with 1000
    markers. If it doesn't on iGPU, consider:
    - Lowering the offscreen RT to half-resolution and upscaling.
    - Frustum-culling markers CPU-side before instance upload.
    - Combining terrain + marker passes into one pipeline with a
      branched shader (overkill; only if numbers force it).

## Step 7 — Exit criteria

- 7 commits on `main`: Phase 1 (offscreen RT), Phase 2 (terrain depth
  migration), Phase 3 (camera + alpha QoL), Phase 4 (GPU markers
  pipeline + MarkerBatch), Phase 5 (symmetry axis lines), Phase 6
  (label projection), Phase 7 (rollup).
- 1 devlog folder filled.
- ADR-037 in `docs/DECISIONS.md` with the full architectural rationale.
- SRS / ROADMAP STATUS UPDATEs.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green at each commit.
- Smoke test (record in final devlog log, screenshots in `notes.md`):
  - Editor launches; central viewport shows terrain (no visual
    regression vs pre-sprint).
  - Place 4 start positions at the corners of a 16-SMU map. Orbit
    360°. Verify markers stay correctly depth-ordered: the closest
    marker is always on top of the next-nearest.
  - Sculpt a 200-elmo-tall hill between two start positions. Orbit
    so the camera looks at the hill from behind one of the markers.
    Verify the second marker (behind the hill) is occluded by the
    hill — does NOT show through.
  - With Symmetry::Quad on, paint a brush stroke. The four ghost
    rings should depth-sort correctly when orbiting.
  - Cross-tool mode (Tool::Select with markers visible) — markers
    render at 50 % alpha; blend correctly when overlapping.
  - Resize the window from 1024×768 to 1920×1080 and back. No green
    flash; no missing markers; no validation errors in the log.
  - `cargo test --workspace -- render markers` runs green
    (new tests added: `MarkerBatch::sort_back_to_front` invariants,
    `OrbitCamera::near_far` formula, `OffscreenTarget::resize`
    idempotency).
- Final devlog log:
  - Summary of what shipped.
  - Timing measurements: terrain pass ms / marker pass ms / composite
    ms at 16 SMU + 100 markers + 4096² heightmap.
  - Any wgpu API quirks (eframe / egui-wgpu version notes).
  - "Sprint 14 = Water + Lava (C9)" handoff per the new
    order-of-attack table in `phase-3-plan.md`. The water plane MVP
    drops onto the depth/alpha foundation this sprint just built.

Start by reading `render.rs` and `terrain.wgsl` end-to-end while
sketching the offscreen RT lifecycle on paper. Don't touch any code
until you understand the
`Callback::prepare` / `Callback::paint` split — that's the linchpin
of the offscreen-RT pattern. Most online wgpu+egui tutorials assume
the Callback's `paint()` is where rendering happens; THIS sprint uses
`prepare()` instead because we want depth attachment that egui-wgpu's
internal pass doesn't provide. Once that's clear, Phase 1 is
mechanical.

The biggest risk is Phase 4 — the marker pipeline + batch. Build it
incrementally: get one filled-circle rendering at a fixed position
first. Then make it instanced. Then sort. Then add the shape
variants. Tests at each step.
