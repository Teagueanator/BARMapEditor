//! Top-right nav-gizmo (3-axis compass) for the central viewport.
//!
//! Paints a small (~80 px) widget that shows the world basis vectors
//! projected through the current camera. Clicking an axis snaps the
//! camera to look along that axis; dragging inside the gizmo orbits.
//!
//! UX research digest §6 "Discoverability of camera controls" — this
//! is one of three affordances (gizmo + first-launch hint + `?`
//! cheat-sheet) that B3 ships in lockstep.

use eframe::egui;
use glam::Vec2 as GVec2;
use glam::Vec3 as GVec3;

use crate::render::OrbitCamera;

/// Gizmo square edge length in screen pixels.
pub const GIZMO_SIZE: f32 = 80.0;
/// Axis arrow length from the gizmo centre.
pub const GIZMO_RADIUS: f32 = 28.0;
/// Click hit-test radius around an axis label.
pub const LABEL_HIT_PX: f32 = 16.0;
/// Padding from the central rect's edges.
pub const GIZMO_PADDING: f32 = 8.0;

/// One of six world-axis directions the gizmo exposes. Click → snap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisDir {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl AxisDir {
    /// All six axes in display order.
    pub const ALL: [AxisDir; 6] = [
        AxisDir::PosX,
        AxisDir::NegX,
        AxisDir::PosY,
        AxisDir::NegY,
        AxisDir::PosZ,
        AxisDir::NegZ,
    ];

    /// One-character label rendered at the arrow tip.
    pub fn label(self) -> &'static str {
        match self {
            AxisDir::PosX => "+X",
            AxisDir::NegX => "-X",
            AxisDir::PosY => "+Y",
            AxisDir::NegY => "-Y",
            AxisDir::PosZ => "+Z",
            AxisDir::NegZ => "-Z",
        }
    }

    /// Unit world-space direction.
    pub fn world(self) -> GVec3 {
        match self {
            AxisDir::PosX => GVec3::new(1.0, 0.0, 0.0),
            AxisDir::NegX => GVec3::new(-1.0, 0.0, 0.0),
            AxisDir::PosY => GVec3::new(0.0, 1.0, 0.0),
            AxisDir::NegY => GVec3::new(0.0, -1.0, 0.0),
            AxisDir::PosZ => GVec3::new(0.0, 0.0, 1.0),
            AxisDir::NegZ => GVec3::new(0.0, 0.0, -1.0),
        }
    }

    /// Colour: red for X, green for Y, blue for Z. Negative variants
    /// render dimmer (50 % alpha-mul).
    pub fn color(self) -> egui::Color32 {
        match self {
            AxisDir::PosX => egui::Color32::from_rgb(220, 90, 90),
            AxisDir::NegX => egui::Color32::from_rgb(150, 70, 70),
            AxisDir::PosY => egui::Color32::from_rgb(80, 200, 100),
            AxisDir::NegY => egui::Color32::from_rgb(60, 130, 70),
            AxisDir::PosZ => egui::Color32::from_rgb(100, 160, 230),
            AxisDir::NegZ => egui::Color32::from_rgb(70, 110, 160),
        }
    }

    /// Camera `(yaw, pitch)` for "look along this axis toward map
    /// centre." Top / bottom views clamp pitch away from the gimbal
    /// singularity (`±π/2`) by `0.05` rad so the orbit math stays
    /// well-behaved.
    pub fn camera_snap(self) -> (f32, f32) {
        // OrbitCamera convention (see `render::OrbitCamera::eye`):
        //   eye = target + (cos(p)·sin(y),  sin(p),  cos(p)·cos(y))·d
        // The camera sits at `eye` and looks toward `target`. To "look
        // along +X" we want the eye on the +X side of the target, i.e.
        // direction vector (1, 0, 0) from target. Solve:
        //   cos(p)·sin(y) = 1, sin(p) = 0, cos(p)·cos(y) = 0
        //   → p = 0, y = π/2.
        use std::f32::consts::FRAC_PI_2;
        let near_top = FRAC_PI_2 - 0.05;
        match self {
            AxisDir::PosX => (FRAC_PI_2, 0.0),
            AxisDir::NegX => (-FRAC_PI_2, 0.0),
            AxisDir::PosY => (0.0, near_top),
            AxisDir::NegY => (0.0, -near_top),
            AxisDir::PosZ => (0.0, 0.0),
            AxisDir::NegZ => (std::f32::consts::PI, 0.0),
        }
    }
}

/// Pure helper: project a unit world-space axis through the camera's
/// view matrix and return the screen-space direction (unit vector).
///
/// We project two world points — the camera's `target` and
/// `target + world_axis` — through the same MVP at a synthetic
/// `(width, height)` rect, then take their delta. The result is
/// invariant to the gizmo's screen position (translation in NDC is a
/// constant added to both projections).
///
/// If the axis projects to a degenerate (zero-length) screen vector
/// (e.g. axis aligned with the view direction), returns `None`.
pub fn axis_screen_direction(camera: &OrbitCamera, axis: GVec3) -> Option<GVec2> {
    // Synthetic square viewport — only directions matter; aspect ratio
    // is irrelevant as long as we use the same matrix for both points.
    let aspect = 1.0_f32;
    let vp = camera.view_proj_matrix(aspect);

    let project = |p: GVec3| {
        let clip = vp * p.extend(1.0);
        if clip.w.abs() < 1e-6 {
            return None;
        }
        // egui screen-y grows downward; flip the y of NDC.
        Some(GVec2::new(clip.x / clip.w, -clip.y / clip.w))
    };

    let a = project(camera.target)?;
    let b = project(camera.target + axis)?;
    let d = b - a;
    if d.length() < 1e-6 {
        None
    } else {
        Some(d.normalize())
    }
}

/// Pure helper: compute the gizmo's anchor rect (top-right of the
/// central viewport, with `GIZMO_PADDING` from the edges).
pub fn gizmo_rect(viewport: egui::Rect) -> egui::Rect {
    let size = egui::Vec2::splat(GIZMO_SIZE);
    let min = egui::Pos2::new(
        viewport.max.x - GIZMO_SIZE - GIZMO_PADDING,
        viewport.min.y + GIZMO_PADDING,
    );
    egui::Rect::from_min_size(min, size)
}

/// Pure helper: screen-space label tip position for an axis, given
/// the gizmo centre + screen direction (from
/// [`axis_screen_direction`]).
pub fn label_tip(centre: egui::Pos2, dir: GVec2) -> egui::Pos2 {
    egui::Pos2::new(
        centre.x + dir.x * GIZMO_RADIUS,
        centre.y + dir.y * GIZMO_RADIUS,
    )
}

/// Pure helper: hit-test which axis (if any) the cursor is over.
///
/// Computes each axis's projected screen direction, derives the label
/// tip position, and returns the nearest axis within `LABEL_HIT_PX`.
/// Returns `None` if the cursor isn't near any tip — caller should
/// fall back to the gizmo-rect drag-orbit path.
pub fn hit_test_axis(
    cursor: egui::Pos2,
    centre: egui::Pos2,
    camera: &OrbitCamera,
) -> Option<AxisDir> {
    let mut best: Option<(AxisDir, f32)> = None;
    for &axis in &AxisDir::ALL {
        let Some(dir) = axis_screen_direction(camera, axis.world()) else {
            continue;
        };
        let tip = label_tip(centre, dir);
        let d = (cursor - tip).length();
        if d <= LABEL_HIT_PX && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((axis, d));
        }
    }
    best.map(|(a, _)| a)
}

/// Paint the gizmo into `viewport`'s top-right corner. Returns the
/// gizmo's screen rect so the caller can do its own hit testing for
/// drag-orbit (a click inside the rect but not on an axis tip
/// initiates an orbit drag — same math as the central rect's RMB).
pub fn paint_nav_gizmo(
    painter: &egui::Painter,
    viewport: egui::Rect,
    camera: &OrbitCamera,
) -> egui::Rect {
    let rect = gizmo_rect(viewport);
    let centre = rect.center();

    // Faint background disc so the gizmo reads as a distinct widget
    // even over bright terrain.
    painter.circle_filled(
        centre,
        GIZMO_RADIUS + 6.0,
        egui::Color32::from_rgba_premultiplied(40, 40, 40, 160),
    );
    painter.circle_stroke(
        centre,
        GIZMO_RADIUS + 6.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_premultiplied(100, 100, 100, 220),
        ),
    );

    let font = egui::FontId::proportional(10.0);
    for &axis in &AxisDir::ALL {
        let Some(dir) = axis_screen_direction(camera, axis.world()) else {
            continue;
        };
        let tip = label_tip(centre, dir);
        let color = axis.color();
        // Arrow line.
        painter.line_segment([centre, tip], egui::Stroke::new(2.0, color));
        // Tip disc + label.
        painter.circle_filled(tip, 7.0, color);
        painter.text(
            tip,
            egui::Align2::CENTER_CENTER,
            axis.label(),
            font.clone(),
            egui::Color32::BLACK,
        );
    }

    rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_4;

    fn default_camera() -> OrbitCamera {
        OrbitCamera::framing(8192.0, 8192.0)
    }

    // ---------------------- AxisDir helpers ----------------------

    #[test]
    fn all_axes_have_distinct_labels() {
        let mut seen = std::collections::HashSet::new();
        for &a in &AxisDir::ALL {
            assert!(seen.insert(a.label()), "duplicate label {}", a.label());
        }
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn all_axes_have_distinct_world_vectors() {
        let mut seen = Vec::new();
        for &a in &AxisDir::ALL {
            let w = a.world();
            for prev in &seen {
                assert_ne!(*prev, w, "duplicate world axis");
            }
            seen.push(w);
        }
    }

    #[test]
    fn world_vectors_are_unit_length() {
        for &a in &AxisDir::ALL {
            let len = a.world().length();
            assert!((len - 1.0).abs() < 1e-6, "axis {} not unit", a.label());
        }
    }

    #[test]
    fn negative_pairs_are_anti_parallel() {
        assert_eq!(AxisDir::PosX.world(), -AxisDir::NegX.world());
        assert_eq!(AxisDir::PosY.world(), -AxisDir::NegY.world());
        assert_eq!(AxisDir::PosZ.world(), -AxisDir::NegZ.world());
    }

    #[test]
    fn axis_colors_have_correct_dominance() {
        // X = red dominant; Y = green dominant; Z = blue dominant.
        let cx = AxisDir::PosX.color();
        assert!(cx.r() > cx.g() && cx.r() > cx.b(), "X should be red");
        let cy = AxisDir::PosY.color();
        assert!(cy.g() > cy.r() && cy.g() > cy.b(), "Y should be green");
        let cz = AxisDir::PosZ.color();
        assert!(cz.b() > cz.r() && cz.b() > cz.g(), "Z should be blue");
    }

    // ---------------------- camera_snap ----------------------

    #[test]
    fn camera_snap_pos_x_yaw_is_half_pi_pitch_zero() {
        let (yaw, pitch) = AxisDir::PosX.camera_snap();
        assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(pitch.abs() < 1e-6);
    }

    #[test]
    fn camera_snap_neg_z_yaw_is_pi_pitch_zero() {
        let (yaw, pitch) = AxisDir::NegZ.camera_snap();
        assert!((yaw - std::f32::consts::PI).abs() < 1e-6);
        assert!(pitch.abs() < 1e-6);
    }

    #[test]
    fn camera_snap_pos_y_pitch_near_top_clamped() {
        // Top view pitch must NOT equal ±π/2 (singular) — clamped by 0.05.
        let (_yaw, pitch) = AxisDir::PosY.camera_snap();
        assert!(pitch > 0.0);
        assert!(pitch < std::f32::consts::FRAC_PI_2);
        assert!((std::f32::consts::FRAC_PI_2 - pitch - 0.05).abs() < 1e-6);
    }

    #[test]
    fn camera_snap_neg_y_pitch_near_bottom_clamped() {
        let (_yaw, pitch) = AxisDir::NegY.camera_snap();
        assert!(pitch < 0.0);
        assert!(pitch > -std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn camera_snap_pos_z_is_origin_orientation() {
        let (yaw, pitch) = AxisDir::PosZ.camera_snap();
        assert!(yaw.abs() < 1e-6);
        assert!(pitch.abs() < 1e-6);
    }

    #[test]
    fn camera_snap_pos_x_actually_places_eye_on_pos_x_side() {
        // Build a camera with the snap-target orientation; verify its
        // eye position lands on the +X side of `target`. This pins the
        // sign convention to OrbitCamera::eye for real.
        let mut cam = default_camera();
        let (yaw, pitch) = AxisDir::PosX.camera_snap();
        cam.yaw = yaw;
        cam.pitch = pitch;
        // Compute eye via the same formula `OrbitCamera::eye` uses.
        let (sy, cy) = cam.yaw.sin_cos();
        let (sp, cp) = cam.pitch.sin_cos();
        let dir = GVec3::new(cp * sy, sp, cp * cy);
        let eye = cam.target + dir * cam.distance;
        // Eye should be on the +X side (eye.x > target.x).
        assert!(
            eye.x > cam.target.x + cam.distance * 0.99,
            "eye.x ({}) not on +X side of target.x ({})",
            eye.x,
            cam.target.x
        );
    }

    // ---------------------- axis_screen_direction ----------------------

    #[test]
    fn axis_screen_direction_returns_some_for_x_at_default_camera() {
        let cam = default_camera();
        let dir = axis_screen_direction(&cam, GVec3::new(1.0, 0.0, 0.0));
        assert!(dir.is_some());
        let d = dir.unwrap();
        assert!((d.length() - 1.0).abs() < 1e-4, "not unit length");
    }

    #[test]
    fn axis_screen_direction_x_and_neg_x_are_anti_parallel() {
        let cam = default_camera();
        let dx = axis_screen_direction(&cam, GVec3::new(1.0, 0.0, 0.0)).expect("x");
        let dnx = axis_screen_direction(&cam, GVec3::new(-1.0, 0.0, 0.0)).expect("-x");
        // Anti-parallel within float tolerance.
        let dot = dx.dot(dnx);
        assert!(dot < -0.99, "dot {} not anti-parallel", dot);
    }

    #[test]
    fn axis_screen_direction_y_up_in_world_is_negative_y_on_screen() {
        // egui screen-y grows downward, and the world's +Y is "up" in
        // OrbitCamera's left-handed coordinate space; at default camera
        // orientation the +Y axis should project with a negative screen-y.
        let cam = default_camera();
        let dir = axis_screen_direction(&cam, GVec3::new(0.0, 1.0, 0.0)).expect("y");
        assert!(
            dir.y < 0.0,
            "world +Y projected to screen-y >= 0: {:?}",
            dir
        );
    }

    // ---------------------- gizmo_rect / label_tip ----------------------

    #[test]
    fn gizmo_rect_anchored_to_top_right_with_padding() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(800.0, 600.0));
        let g = gizmo_rect(viewport);
        // Top edge.
        assert!((g.min.y - GIZMO_PADDING).abs() < 1e-6);
        // Right edge: g.max.x + padding = viewport.max.x.
        assert!((g.max.x + GIZMO_PADDING - 800.0).abs() < 1e-6);
        assert!((g.width() - GIZMO_SIZE).abs() < 1e-6);
        assert!((g.height() - GIZMO_SIZE).abs() < 1e-6);
    }

    #[test]
    fn label_tip_offsets_by_gizmo_radius_along_direction() {
        let centre = egui::Pos2::new(100.0, 100.0);
        let tip = label_tip(centre, GVec2::new(1.0, 0.0));
        assert!((tip.x - (100.0 + GIZMO_RADIUS)).abs() < 1e-6);
        assert!((tip.y - 100.0).abs() < 1e-6);
    }

    // ---------------------- hit_test_axis ----------------------

    #[test]
    fn hit_test_returns_none_when_cursor_far_from_centre() {
        let cam = default_camera();
        let centre = egui::Pos2::new(100.0, 100.0);
        let far = egui::Pos2::new(10000.0, 10000.0);
        assert_eq!(hit_test_axis(far, centre, &cam), None);
    }

    #[test]
    fn hit_test_returns_some_when_cursor_at_x_tip() {
        let cam = default_camera();
        let centre = egui::Pos2::new(100.0, 100.0);
        // Compute the +X tip and place the cursor exactly there.
        let dir = axis_screen_direction(&cam, AxisDir::PosX.world()).expect("x dir");
        let tip = label_tip(centre, dir);
        let hit = hit_test_axis(tip, centre, &cam);
        assert!(hit.is_some());
    }

    #[test]
    fn hit_test_prefers_closer_axis_when_two_tips_are_near() {
        let cam = default_camera();
        let centre = egui::Pos2::new(100.0, 100.0);
        // Cursor exactly on the +Y tip — should resolve to PosY.
        let dir = axis_screen_direction(&cam, AxisDir::PosY.world()).expect("y dir");
        let tip = label_tip(centre, dir);
        assert_eq!(hit_test_axis(tip, centre, &cam), Some(AxisDir::PosY));
    }

    // ---------------------- camera-orientation invariants ----------------------

    #[test]
    fn axis_screen_directions_rotate_with_camera_yaw() {
        // Rotating the camera yaw by 90° should rotate the +X screen
        // projection too — it's not constant. This is a regression
        // guard for "I forgot to recompute the gizmo when the camera
        // moved."
        let mut cam = default_camera();
        let d0 = axis_screen_direction(&cam, GVec3::new(1.0, 0.0, 0.0)).expect("x@0");
        cam.yaw += FRAC_PI_4;
        let d1 = axis_screen_direction(&cam, GVec3::new(1.0, 0.0, 0.0)).expect("x@π/4");
        // Different orientations should produce different screen
        // directions (allowing small float wiggle, demanding > 5° diff).
        let dot = d0.dot(d1).clamp(-1.0, 1.0);
        let angle = dot.acos();
        assert!(angle > 0.05, "expected rotation, got angle {}", angle);
    }
}
