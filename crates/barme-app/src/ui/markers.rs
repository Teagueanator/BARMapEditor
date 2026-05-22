//! Marker batch + GPU instance encoding (Sprint 13 / ADR-037).
//!
//! `central()` builds a fresh [`MarkerBatch`] every frame, pushes one
//! [`Marker`] per visible UI element (start positions, metal spots,
//! geo vents, brush rings, mirror ghosts), then hands the sorted +
//! GPU-encoded result to [`crate::render::TerrainCallback`] which
//! uploads it as a storage buffer and draws via the marker pipeline.
//!
//! The pipeline depth-tests against the terrain (so a marker behind a
//! hill is occluded) but doesn't write depth itself; correct
//! translucent blending therefore depends on the CPU-side back-to-front
//! sort in [`MarkerBatch::sort_back_to_front`].
//!
//! **Multithreading:** for marker counts <10k (Sprint 13's foreseeable
//! load), the stable single-threaded `sort_by` is sub-millisecond. The
//! user OKed multithreading where it pays — swap to
//! `rayon::par_sort_by_cached_key` if the final-devlog perf table ever
//! shows the sort exceeding ~1 ms.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

/// Vertical lift (in elmos) applied to every marker's world Y before
/// projection so a marker at `y = 0` doesn't z-fight terrain at the
/// same elevation (pitfall #10).
///
/// Sprint 19 — bumped 2.0 → 32.0. With `height_scale` defaulting to
/// 256 elmos, a 2-elmo lift was ~0.8% of typical map relief; the
/// screen-space marker sprite is a flat quad and its lower edge
/// routinely fell below visible terrain at shallow viewing angles,
/// burying metal / vent / start-pos / feature markers. 32 elmos is
/// ~12% of the default height range — comfortable headroom regardless
/// of view angle without divorcing the marker from its XZ anchor.
pub const MARKER_Y_LIFT_ELMOS: f32 = 32.0;

/// Visual shape variants supported by the Sprint-13 marker shader.
/// IDs are mapped to the WGSL `Instance.shape_id` field by
/// [`MarkerShape::shape_id`]; renumbering here MUST be mirrored in
/// `markers.wgsl::fs_main`.
///
/// Sprint 29 / ADR-046 — `TexturedSprite` samples the per-family
/// decal atlas (a `texture_2d_array` bound at `@group(0)
/// @binding(2)`). The atlas layer is carried as a per-instance
/// `u32` next to `shape_id`; see [`MarkerInstanceGpu::texture_layer`].
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MarkerShape {
    /// Anti-aliased filled disk.
    FilledCircle,
    /// Anti-aliased outline ring at ~0.85..1.0 of the radius.
    OutlineRing,
    /// Filled body with a white outer ring — start-position glyph.
    FilledWithStroke,
    /// Upward-pointing equilateral triangle — geo-vent glyph.
    Triangle,
    /// Outline-only upward triangle — geo-vent mirror glyph (Phase 5).
    /// The inner ~75 %-scale triangle is discarded so only the ring
    /// remains. Kept on the marker pipeline (not the line pipeline)
    /// so it stays screen-space sized — matches the primary Triangle's
    /// size at every camera distance.
    OutlineTriangle,
    /// Sample the feature decal atlas at `layer`. The atlas is
    /// uploaded by [`crate::feature_decals`] at app startup; the
    /// per-family layer index is resolved by the
    /// `FeatureDecalRegistry`. Unknown layers (e.g. when a family
    /// has no upstream diffuse) fall back to the category glyph
    /// before the marker is pushed — by the time the GPU sees a
    /// `TexturedSprite` the layer is guaranteed to be populated.
    TexturedSprite { layer: u32 },
}

impl MarkerShape {
    pub fn shape_id(self) -> u32 {
        match self {
            MarkerShape::FilledCircle => 0,
            MarkerShape::OutlineRing => 1,
            MarkerShape::FilledWithStroke => 2,
            MarkerShape::Triangle => 3,
            MarkerShape::OutlineTriangle => 4,
            MarkerShape::TexturedSprite { .. } => 5,
        }
    }

    /// Per-instance texture array layer. Non-`TexturedSprite`
    /// shapes return 0 — the fragment shader's case for those
    /// shapes never consults the field, so the value is inert.
    pub fn texture_layer(self) -> u32 {
        match self {
            MarkerShape::TexturedSprite { layer } => layer,
            _ => 0,
        }
    }
}

/// A single marker queued for the next frame. `color` is an
/// `egui::Color32` so call sites don't have to remember the
/// premultiplied byte convention — `egui::Color32` is internally
/// premultiplied.
#[derive(Copy, Clone, Debug)]
pub struct Marker {
    /// World-space position. The Y-lift is applied at encode time, not
    /// at push time, so call sites can keep using the natural y=0
    /// position for ground-pinned markers.
    pub world_pos: Vec3,
    /// Screen-space radius in *logical* pixels. The marker shader
    /// converts to clip-space via `viewport_size`, so the marker stays
    /// pixel-sized regardless of depth or DPI.
    pub radius_px: f32,
    pub color: egui::Color32,
    pub shape: MarkerShape,
}

/// Frame-local accumulator. Built fresh in `central()` every frame —
/// never persists across frames (pitfall #14: marker positions track
/// the cursor each frame).
#[derive(Default, Debug, Clone)]
pub struct MarkerBatch {
    items: Vec<Marker>,
}

/// GPU-layout instance struct. Must match `markers.wgsl::Instance`
/// byte-for-byte. Storage-buffer layout (not std140) — vec3 + float
/// = 16 B, vec4 = 16 B, u32 + u32 + 2 × u32 pad = 16 B.
///
/// Sprint 29 / ADR-046 — `texture_layer` consumes one slot of the
/// former 3-u32 pad. Struct size and alignment are unchanged (48 B);
/// the WGSL `Instance` mirror at `markers.wgsl::Instance` follows
/// the same renumbering.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, Debug)]
pub struct MarkerInstanceGpu {
    /// World position with [`MARKER_Y_LIFT_ELMOS`] already added.
    pub world_pos: [f32; 3],
    pub radius_px: f32,
    /// Premultiplied RGBA in `[0, 1]`.
    pub color: [f32; 4],
    pub shape_id: u32,
    /// Layer index in the marker decal `texture_2d_array` — only
    /// consulted when `shape_id == 5` (TexturedSprite).
    pub texture_layer: u32,
    pub _pad: [u32; 2],
}

impl MarkerBatch {
    pub fn push(&mut self, m: Marker) {
        self.items.push(m);
    }

    #[allow(dead_code)] // future bulk-push API; brush ghost helpers may grow into it
    pub fn extend(&mut self, it: impl IntoIterator<Item = Marker>) {
        self.items.extend(it);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[allow(dead_code)] // useful for callers; kept symmetric with len()
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Sort the batch back-to-front using the *view* matrix (NOT
    /// view-proj). Left-handed view space (glam's
    /// `Mat4::look_at_lh`) places larger Z farther from the camera, so
    /// we sort *descending* by view-Z — farthest marker drawn first,
    /// closest drawn last and therefore on top.
    ///
    /// Stable sort: two markers at identical view-space Z keep their
    /// insertion order. This is what lets `central()` push the
    /// "black halo" before the "coloured fill" of a brush centre dot
    /// and trust the smaller fill to land on top.
    pub fn sort_back_to_front(&mut self, view: Mat4) {
        self.items.sort_by(|a, b| {
            let za = (view * a.world_pos.extend(1.0)).z;
            let zb = (view * b.world_pos.extend(1.0)).z;
            // Descending: farther (larger view-Z in LH) first.
            zb.partial_cmp(&za).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Consume the batch and emit GPU-ready instances. Applies the
    /// world-Y lift (pitfall #10) and expands the premultiplied
    /// `Color32` bytes into `[0, 1]` floats.
    pub fn into_instances(self) -> Vec<MarkerInstanceGpu> {
        let inv = 1.0 / 255.0;
        self.items
            .into_iter()
            .map(|m| {
                let [r, g, b, a] = m.color.to_array();
                MarkerInstanceGpu {
                    world_pos: [
                        m.world_pos.x,
                        m.world_pos.y + MARKER_Y_LIFT_ELMOS,
                        m.world_pos.z,
                    ],
                    radius_px: m.radius_px,
                    color: [
                        r as f32 * inv,
                        g as f32 * inv,
                        b as f32 * inv,
                        a as f32 * inv,
                    ],
                    shape_id: m.shape.shape_id(),
                    texture_layer: m.shape.texture_layer(),
                    _pad: [0, 0],
                }
            })
            .collect()
    }
}

/// CPU mirror of the marker shader's vertex projection. Returns the
/// projected screen position for `world` — `None` *only* when the point
/// is behind the camera (`clip.w <= 0`). Off-screen points still
/// return projected positions; the caller clips via
/// `ui.painter_at(rect)`.
///
/// Used by `central()` to project text labels (which stay in
/// egui::Painter — wgpu SDF text is out of scope for Sprint 13) so the
/// labels always land at the same pixel as the GPU marker they
/// annotate (Phase 6).
#[allow(dead_code)] // wired Phase 6 by label projection
pub fn project_to_screen(
    world: Vec3,
    view_proj: Mat4,
    viewport_size: Vec2,
    rect_min: Vec2,
) -> Option<Vec2> {
    let clip = view_proj * world.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc_x = clip.x / clip.w;
    let ndc_y = clip.y / clip.w;
    Some(Vec2::new(
        rect_min.x + (ndc_x + 1.0) * 0.5 * viewport_size.x,
        rect_min.y + (1.0 - ndc_y) * 0.5 * viewport_size.y,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Mat4, Vec3};

    fn lh_view_camera_at_origin_looking_along_pos_z() -> Mat4 {
        // Camera at origin, looking down +Z (left-handed). World +Z is
        // away from camera; LH view space puts +Z = farther. Helper
        // mirrors the OrbitCamera::view_matrix call site so the test
        // exercises the same convention as production.
        Mat4::look_at_lh(Vec3::ZERO, Vec3::new(0.0, 0.0, 1.0), Vec3::Y)
    }

    fn marker_at(world: Vec3) -> Marker {
        Marker {
            world_pos: world,
            radius_px: 8.0,
            color: egui::Color32::from_rgba_premultiplied(255, 0, 0, 255),
            shape: MarkerShape::FilledCircle,
        }
    }

    #[test]
    fn empty_batch_is_empty() {
        let b = MarkerBatch::default();
        assert_eq!(b.len(), 0);
        assert!(b.is_empty());
        assert!(b.into_instances().is_empty());
    }

    #[test]
    fn sort_back_to_front_orders_descending_z_in_lh_view() {
        // Three markers at increasing world-Z (LH: farther from camera
        // at origin). After sort, items[0] should be the farthest =
        // largest world-Z.
        let close = marker_at(Vec3::new(0.0, 0.0, 10.0));
        let mid = marker_at(Vec3::new(0.0, 0.0, 100.0));
        let far = marker_at(Vec3::new(0.0, 0.0, 1000.0));

        let mut b = MarkerBatch::default();
        b.push(close);
        b.push(far);
        b.push(mid);
        b.sort_back_to_front(lh_view_camera_at_origin_looking_along_pos_z());

        let zs: Vec<f32> = b.items.iter().map(|m| m.world_pos.z).collect();
        assert_eq!(
            zs,
            vec![1000.0, 100.0, 10.0],
            "back-to-front: farthest (largest Z) first"
        );
    }

    #[test]
    fn sort_is_stable_for_equal_view_z() {
        // Two markers at the same world position keep insertion order.
        // Used to layer (black halo, coloured fill) without flicker.
        let halo = Marker {
            color: egui::Color32::BLACK,
            ..marker_at(Vec3::new(50.0, 0.0, 50.0))
        };
        let fill = Marker {
            color: egui::Color32::from_rgba_premultiplied(0, 200, 0, 255),
            radius_px: 4.0,
            ..marker_at(Vec3::new(50.0, 0.0, 50.0))
        };

        let mut b = MarkerBatch::default();
        b.push(halo);
        b.push(fill);
        b.sort_back_to_front(lh_view_camera_at_origin_looking_along_pos_z());

        assert_eq!(b.items[0].color, egui::Color32::BLACK);
        assert_eq!(b.items[1].radius_px, 4.0);
    }

    #[test]
    fn into_instances_applies_y_lift() {
        // A marker pushed at y=0 should arrive at the GPU with
        // y = MARKER_Y_LIFT_ELMOS to avoid z-fight (pitfall #10).
        let mut b = MarkerBatch::default();
        b.push(marker_at(Vec3::new(123.0, 0.0, 456.0)));
        let inst = b.into_instances();
        assert_eq!(inst.len(), 1);
        assert_eq!(inst[0].world_pos[0], 123.0);
        assert_eq!(inst[0].world_pos[1], MARKER_Y_LIFT_ELMOS);
        assert_eq!(inst[0].world_pos[2], 456.0);
    }

    #[test]
    fn into_instances_encodes_premul_color_as_zero_to_one() {
        // 50%-alpha pure red, premultiplied by egui: rgb = (128, 0, 0),
        // a = 128. Expand to [0,1] floats.
        let red_50 = egui::Color32::from_rgba_unmultiplied(255, 0, 0, 128);
        let mut b = MarkerBatch::default();
        b.push(Marker {
            color: red_50,
            ..marker_at(Vec3::ZERO)
        });
        let inst = b.into_instances();
        // egui internally premultiplies: r = 255 * 128/255 = 128.
        let expected = [128.0 / 255.0, 0.0, 0.0, 128.0 / 255.0];
        for (i, e) in expected.iter().enumerate() {
            assert!(
                (inst[0].color[i] - e).abs() < 1e-4,
                "channel {i}: got {}, expected {}",
                inst[0].color[i],
                e,
            );
        }
    }

    #[test]
    fn instance_struct_is_48_bytes() {
        // Matches markers.wgsl::Instance storage-buffer layout. A
        // change here means the WGSL needs updating too.
        assert_eq!(std::mem::size_of::<MarkerInstanceGpu>(), 48);
    }

    #[test]
    fn shape_id_mapping_is_pinned() {
        // The WGSL switch matches these values verbatim; renumbering
        // here without updating markers.wgsl is a silent bug.
        assert_eq!(MarkerShape::FilledCircle.shape_id(), 0);
        assert_eq!(MarkerShape::OutlineRing.shape_id(), 1);
        assert_eq!(MarkerShape::FilledWithStroke.shape_id(), 2);
        assert_eq!(MarkerShape::Triangle.shape_id(), 3);
        assert_eq!(MarkerShape::OutlineTriangle.shape_id(), 4);
        assert_eq!(MarkerShape::TexturedSprite { layer: 0 }.shape_id(), 5);
    }

    #[test]
    fn texture_layer_routing_only_for_textured_sprite() {
        // Sprint 29 / ADR-046 — non-TexturedSprite shapes report
        // texture_layer == 0 so the fragment shader's other branches
        // ignore the field cleanly. TexturedSprite carries the array
        // index through to the GPU.
        assert_eq!(MarkerShape::FilledCircle.texture_layer(), 0);
        assert_eq!(MarkerShape::OutlineRing.texture_layer(), 0);
        assert_eq!(MarkerShape::Triangle.texture_layer(), 0);
        assert_eq!(MarkerShape::TexturedSprite { layer: 7 }.texture_layer(), 7);
        assert_eq!(
            MarkerShape::TexturedSprite { layer: 31 }.texture_layer(),
            31
        );
    }

    #[test]
    fn into_instances_routes_texture_layer_from_shape() {
        let mut b = MarkerBatch::default();
        b.push(Marker {
            shape: MarkerShape::TexturedSprite { layer: 11 },
            ..marker_at(Vec3::new(1.0, 0.0, 2.0))
        });
        b.push(Marker {
            shape: MarkerShape::FilledCircle,
            ..marker_at(Vec3::new(3.0, 0.0, 4.0))
        });
        let inst = b.into_instances();
        assert_eq!(inst[0].shape_id, 5);
        assert_eq!(inst[0].texture_layer, 11);
        assert_eq!(inst[1].shape_id, 0);
        assert_eq!(
            inst[1].texture_layer, 0,
            "non-TexturedSprite must zero the layer slot"
        );
    }

    #[test]
    fn marker_y_lift_constant_in_safe_range() {
        // Sprint 19 bumped 2.0 → 32.0 so screen-space marker sprites
        // clear typical terrain relief at shallow viewing angles.
        // The pin keeps the lift in a reasonable band — too small and
        // markers get buried again; too large and they detach
        // visually from their XZ anchor.
        assert!(
            (16.0..=64.0).contains(&MARKER_Y_LIFT_ELMOS),
            "lift {MARKER_Y_LIFT_ELMOS} elmos outside safe range",
        );
    }

    #[test]
    fn project_to_screen_returns_none_for_behind_camera_point() {
        // Camera at origin looking +Z. A point at z=-10 is behind the
        // camera (clip.w <= 0 after LH projection).
        let view = lh_view_camera_at_origin_looking_along_pos_z();
        let proj = Mat4::perspective_lh(60f32.to_radians(), 4.0 / 3.0, 1.0, 10000.0);
        let vp = proj * view;
        let behind = Vec3::new(0.0, 0.0, -10.0);
        let r = project_to_screen(behind, vp, Vec2::new(800.0, 600.0), Vec2::ZERO);
        assert!(r.is_none(), "behind-camera should project to None");
    }

    #[test]
    fn project_to_screen_returns_some_for_off_screen_point() {
        // Phase 6 semantics: off-screen points (clip-space NDC outside
        // [-1, 1]) still return Some — caller clips via painter_at.
        let view = lh_view_camera_at_origin_looking_along_pos_z();
        let proj = Mat4::perspective_lh(60f32.to_radians(), 4.0 / 3.0, 1.0, 10_000.0);
        let vp = proj * view;
        // Point well off the right edge but in front of the camera.
        let way_right = Vec3::new(10_000.0, 0.0, 100.0);
        let r = project_to_screen(way_right, vp, Vec2::new(800.0, 600.0), Vec2::ZERO);
        assert!(
            r.is_some(),
            "off-screen-but-in-front should project to Some"
        );
        // And the x should be well past 800 (off the right side).
        assert!(
            r.unwrap().x > 800.0,
            "off-screen-right should project past the rect width"
        );
    }

    #[test]
    fn project_to_screen_at_rect_origin_offsets_correctly() {
        // A point exactly at camera target should land at rect_min +
        // viewport/2. Offsetting `rect_min` should slide the result
        // by the same vector — confirms rect_min is added correctly.
        let target = Vec3::new(0.0, 0.0, 100.0);
        let view = Mat4::look_at_lh(Vec3::ZERO, target, Vec3::Y);
        let proj = Mat4::perspective_lh(60f32.to_radians(), 1.0, 1.0, 10000.0);
        let vp = proj * view;
        let viewport = Vec2::new(800.0, 600.0);

        let a = project_to_screen(target, vp, viewport, Vec2::ZERO).unwrap();
        let b = project_to_screen(target, vp, viewport, Vec2::new(100.0, 50.0)).unwrap();
        assert!((b.x - (a.x + 100.0)).abs() < 1e-3);
        assert!((b.y - (a.y + 50.0)).abs() < 1e-3);
    }
}
