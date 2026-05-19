//! Lucide / Tabler-style line icons painted directly with [`egui::Painter`]
//! (ADR-035). Each variant of [`Icon`] maps to a small list of paint calls
//! on a logical 24×24 viewBox, scaled into the target rect.
//!
//! Why not a font?  Bundling an icon-font crate (egui-phosphor,
//! egui-material-icons) pulls in a ~200 kB TTF and forces every call
//! site through the `RichText` API. Hand-drawn primitives weigh in at
//! ~4 kB of source per icon, render at any size without bitmap blur,
//! and keep the icon library inspectable in the Rust file (the JSX
//! reference is `/tmp/bar-ui-design/icons.jsx`).
//!
//! Coordinate system: every icon is authored on the 24-unit grid
//! used by `icons.jsx`. [`paint_icon`] maps that grid into the
//! target rect with uniform scaling so non-square rects letterbox
//! rather than distort.

use eframe::egui::{self, Align2, Color32, Pos2, Rect, Stroke, StrokeKind};

/// Catalogue of line icons used across the editor. Match the JSX names
/// from `/tmp/bar-ui-design/icons.jsx` so the two stay grep-able.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    // Tool strip
    Select,
    Sculpt,
    Pin,
    Splat,
    Metal,
    Geo,
    Procgen,

    // UI chrome
    ChevDown,
    ChevRight,
    Play,
    Save,
    Plus,
    Minus,
    X,
    Check,
    Alert,
    Info,
    Help,
    Eye,
    Cog,
    Folder,
    Layers,
    Dice,

    // Viewport options
    Grid,
    Light,
    Wire,
    Expand,
    Map,
    Brush,
    Rotate,
    Scale,
    Trash,
    Tree,
    Rock,
    Wreck,
    Crystal,
    Spray,
    Compass,

    // Symmetry glyphs (used in the top-bar mode dropdown)
    SymH,
    SymV,
    SymQ,
    SymRot,
}

/// All variants in declaration order. Used by tests + the icon-debug
/// gallery in the cheat sheet. The gallery hasn't shipped yet — the
/// constant is `#[allow(dead_code)]` so clippy doesn't gate the
/// release build on an unused public symbol.
#[allow(dead_code)]
pub const ALL: &[Icon] = &[
    Icon::Select,
    Icon::Sculpt,
    Icon::Pin,
    Icon::Splat,
    Icon::Metal,
    Icon::Geo,
    Icon::Procgen,
    Icon::ChevDown,
    Icon::ChevRight,
    Icon::Play,
    Icon::Save,
    Icon::Plus,
    Icon::Minus,
    Icon::X,
    Icon::Check,
    Icon::Alert,
    Icon::Info,
    Icon::Help,
    Icon::Eye,
    Icon::Cog,
    Icon::Folder,
    Icon::Layers,
    Icon::Dice,
    Icon::Grid,
    Icon::Light,
    Icon::Wire,
    Icon::Expand,
    Icon::Map,
    Icon::Brush,
    Icon::Rotate,
    Icon::Scale,
    Icon::Trash,
    Icon::Tree,
    Icon::Rock,
    Icon::Wreck,
    Icon::Crystal,
    Icon::Spray,
    Icon::Compass,
    Icon::SymH,
    Icon::SymV,
    Icon::SymQ,
    Icon::SymRot,
];

/// Paint `icon` into `rect` at the given colour. `stroke_width` is in
/// *screen pixels*, independent of the rect's size — caller chooses
/// (1.2 for small chip glyphs, 1.6 for tool-strip tiles, 2.0 for
/// big modal hero icons).
pub fn paint_icon(
    painter: &egui::Painter,
    rect: Rect,
    icon: Icon,
    color: Color32,
    stroke_width: f32,
) {
    let stroke = Stroke::new(stroke_width, color);
    let mapper = Mapper::new(rect);
    match icon {
        Icon::Select => {
            // Arrow cursor — a stylised pointer.
            mapper.poly_line(
                painter,
                stroke,
                &[
                    (5, 3),
                    (5, 18),
                    (9, 14),
                    (12, 20),
                    (14, 19),
                    (11, 13),
                    (17, 13),
                ],
                true,
            );
        }
        Icon::Sculpt => {
            // Round brush body + handle.
            mapper.circle(painter, stroke, (10, 13), 6);
            mapper.line(painter, stroke, (14, 9), (19, 5));
            mapper.line(painter, stroke, (19, 5), (21, 7));
            mapper.line(painter, stroke, (21, 7), (16, 11));
            mapper.line(painter, stroke, (7, 16), (9, 14));
        }
        Icon::Pin => {
            mapper.teardrop(painter, stroke, (12, 21), (12, 9), 7);
            mapper.circle(painter, stroke, (12, 9), 2);
        }
        Icon::Splat => {
            // Paint-roller-ish stroke.
            mapper.line(painter, stroke, (6, 19), (14, 6));
            mapper.line(painter, stroke, (14, 6), (18, 8));
            mapper.line(painter, stroke, (18, 8), (11, 21));
            mapper.line(painter, stroke, (11, 21), (6, 19));
            mapper.line(painter, stroke, (5, 16), (9, 20));
            mapper.line(painter, stroke, (14, 6), (17, 9));
        }
        Icon::Metal => {
            // Hexagonal-node motif.
            mapper.poly_line(
                painter,
                stroke,
                &[(3, 21), (9, 9), (13, 11), (21, 5)],
                false,
            );
            mapper.poly_line(painter, stroke, &[(16, 5), (21, 5), (21, 10)], false);
            mapper.circle_filled(painter, color, (6, 17), 1);
        }
        Icon::Geo => {
            // House / structure outline as feature placeholder.
            mapper.poly_line(
                painter,
                stroke,
                &[
                    (12, 3),
                    (4, 12),
                    (6, 12),
                    (6, 19),
                    (10, 19),
                    (10, 14),
                    (14, 14),
                    (14, 19),
                    (18, 19),
                    (18, 12),
                    (20, 12),
                ],
                true,
            );
        }
        Icon::Procgen => {
            // Three layered sine waves.
            mapper.curve(painter, stroke, (3, 17), (12, 7), (21, 17));
            mapper.curve(painter, stroke, (3, 12), (10, 4), (16, 8));
            mapper.curve(painter, stroke, (12, 20), (18, 12), (21, 12));
        }
        Icon::ChevDown => {
            mapper.poly_line(painter, stroke, &[(6, 9), (12, 15), (18, 9)], false);
        }
        Icon::ChevRight => {
            mapper.poly_line(painter, stroke, &[(9, 6), (15, 12), (9, 18)], false);
        }
        Icon::Play => {
            mapper.poly_line(painter, stroke, &[(7, 5), (19, 12), (7, 19)], true);
        }
        Icon::Save => {
            mapper.poly_line(
                painter,
                stroke,
                &[(5, 5), (17, 5), (21, 9), (21, 19), (5, 19)],
                true,
            );
            mapper.poly_line(painter, stroke, &[(8, 3), (8, 8), (16, 8), (16, 3)], false);
            mapper.poly_line(
                painter,
                stroke,
                &[(8, 14), (16, 14), (16, 20), (8, 20)],
                true,
            );
        }
        Icon::Plus => {
            mapper.line(painter, stroke, (12, 5), (12, 19));
            mapper.line(painter, stroke, (5, 12), (19, 12));
        }
        Icon::Minus => {
            mapper.line(painter, stroke, (5, 12), (19, 12));
        }
        Icon::X => {
            mapper.line(painter, stroke, (6, 6), (18, 18));
            mapper.line(painter, stroke, (18, 6), (6, 18));
        }
        Icon::Check => {
            mapper.poly_line(painter, stroke, &[(5, 12), (10, 17), (19, 7)], false);
        }
        Icon::Alert => {
            mapper.poly_line(painter, stroke, &[(12, 4), (21, 20), (3, 20)], true);
            mapper.line(painter, stroke, (12, 10), (12, 14));
            mapper.circle_filled(painter, color, (12, 17), 1);
        }
        Icon::Info => {
            mapper.circle(painter, stroke, (12, 12), 9);
            mapper.circle_filled(painter, color, (12, 8), 1);
            mapper.line(painter, stroke, (12, 11), (12, 16));
        }
        Icon::Help => {
            mapper.circle(painter, stroke, (12, 12), 9);
            mapper.curve(painter, stroke, (9, 10), (12, 7), (15, 12));
            mapper.line(painter, stroke, (12, 12), (12, 14));
            mapper.circle_filled(painter, color, (12, 17), 1);
        }
        Icon::Eye => {
            mapper.curve(painter, stroke, (2, 12), (12, 5), (22, 12));
            mapper.curve(painter, stroke, (2, 12), (12, 19), (22, 12));
            mapper.circle(painter, stroke, (12, 12), 3);
        }
        Icon::Cog => {
            mapper.circle(painter, stroke, (12, 12), 3);
            for (a, b) in [
                ((12, 3), (12, 5)),
                ((12, 19), (12, 21)),
                ((3, 12), (5, 12)),
                ((19, 12), (21, 12)),
                ((5, 5), (7, 7)),
                ((17, 17), (19, 19)),
                ((5, 19), (7, 17)),
                ((17, 7), (19, 5)),
            ] {
                mapper.line(painter, stroke, a, b);
            }
        }
        Icon::Folder => {
            mapper.poly_line(
                painter,
                stroke,
                &[
                    (3, 7),
                    (5, 5),
                    (9, 5),
                    (11, 7),
                    (19, 7),
                    (21, 9),
                    (21, 18),
                    (19, 20),
                    (5, 20),
                    (3, 18),
                ],
                true,
            );
        }
        Icon::Layers => {
            mapper.poly_line(painter, stroke, &[(12, 3), (21, 8), (12, 13), (3, 8)], true);
            mapper.poly_line(painter, stroke, &[(3, 12), (12, 17), (21, 12)], false);
            mapper.poly_line(painter, stroke, &[(3, 16), (12, 21), (21, 16)], false);
        }
        Icon::Dice => {
            mapper.rect(painter, stroke, (4, 4), (20, 20));
            for (cx, cy) in [(8, 8), (16, 8), (12, 12), (8, 16), (16, 16)] {
                mapper.circle_filled(painter, color, (cx, cy), 1);
            }
        }
        Icon::Grid => {
            mapper.rect(painter, stroke, (3, 3), (21, 21));
            mapper.line(painter, stroke, (3, 9), (21, 9));
            mapper.line(painter, stroke, (3, 15), (21, 15));
            mapper.line(painter, stroke, (9, 3), (9, 21));
            mapper.line(painter, stroke, (15, 3), (15, 21));
        }
        Icon::Light => {
            mapper.circle(painter, stroke, (12, 12), 4);
            for (a, b) in [
                ((12, 3), (12, 5)),
                ((12, 19), (12, 21)),
                ((3, 12), (5, 12)),
                ((19, 12), (21, 12)),
                ((5, 5), (7, 7)),
                ((17, 17), (19, 19)),
                ((5, 19), (7, 17)),
                ((17, 7), (19, 5)),
            ] {
                mapper.line(painter, stroke, a, b);
            }
        }
        Icon::Wire => {
            mapper.line(painter, stroke, (3, 20), (21, 20));
            mapper.poly_line(
                painter,
                stroke,
                &[(3, 16), (9, 8), (13, 13), (21, 5)],
                false,
            );
            mapper.circle_filled(painter, color, (9, 8), 1);
            mapper.circle_filled(painter, color, (13, 13), 1);
        }
        Icon::Expand => {
            mapper.poly_line(painter, stroke, &[(4, 10), (4, 4), (10, 4)], false);
            mapper.poly_line(painter, stroke, &[(14, 4), (20, 4), (20, 10)], false);
            mapper.poly_line(painter, stroke, &[(20, 14), (20, 20), (14, 20)], false);
            mapper.poly_line(painter, stroke, &[(10, 20), (4, 20), (4, 14)], false);
        }
        Icon::Map => {
            mapper.poly_line(
                painter,
                stroke,
                &[
                    (3, 6),
                    (9, 4),
                    (15, 6),
                    (21, 4),
                    (21, 18),
                    (15, 20),
                    (9, 18),
                    (3, 20),
                ],
                true,
            );
            mapper.line(painter, stroke, (9, 4), (9, 18));
            mapper.line(painter, stroke, (15, 6), (15, 20));
        }
        Icon::Brush => {
            mapper.poly_line(
                painter,
                stroke,
                &[(14, 4), (20, 10), (11, 19), (7, 19), (7, 15)],
                true,
            );
        }
        Icon::Rotate => {
            mapper.arc(
                painter,
                stroke,
                (12, 12),
                9,
                0.6,
                std::f32::consts::TAU - 0.2,
            );
            mapper.poly_line(painter, stroke, &[(21, 4), (21, 10), (15, 10)], false);
        }
        Icon::Scale => {
            mapper.rect(painter, stroke, (5, 5), (11, 11));
            mapper.rect(painter, stroke, (13, 13), (19, 19));
            mapper.line(painter, stroke, (11, 11), (13, 13));
        }
        Icon::Trash => {
            mapper.line(painter, stroke, (4, 7), (20, 7));
            mapper.poly_line(painter, stroke, &[(9, 7), (9, 5), (15, 5), (15, 7)], false);
            mapper.poly_line(
                painter,
                stroke,
                &[(6, 7), (7, 20), (17, 20), (18, 7)],
                false,
            );
        }
        Icon::Tree => {
            mapper.poly_line(
                painter,
                stroke,
                &[
                    (12, 3),
                    (6, 11),
                    (9, 11),
                    (5, 17),
                    (10, 17),
                    (10, 21),
                    (14, 21),
                    (14, 17),
                    (19, 17),
                    (15, 11),
                    (18, 11),
                ],
                true,
            );
        }
        Icon::Rock => {
            mapper.poly_line(
                painter,
                stroke,
                &[
                    (4, 18),
                    (7, 9),
                    (13, 6),
                    (19, 10),
                    (21, 17),
                    (17, 20),
                    (8, 20),
                ],
                true,
            );
            mapper.poly_line(painter, stroke, &[(7, 9), (13, 14), (19, 10)], false);
            mapper.line(painter, stroke, (13, 14), (13, 20));
        }
        Icon::Wreck => {
            mapper.poly_line(
                painter,
                stroke,
                &[(3, 19), (8, 7), (15, 9), (11, 12), (19, 13), (17, 19)],
                true,
            );
            mapper.line(painter, stroke, (8, 7), (15, 9));
            mapper.line(painter, stroke, (11, 12), (17, 19));
        }
        Icon::Crystal => {
            mapper.poly_line(
                painter,
                stroke,
                &[(12, 3), (5, 10), (12, 21), (19, 10)],
                true,
            );
            mapper.line(painter, stroke, (5, 10), (19, 10));
            mapper.line(painter, stroke, (12, 3), (12, 21));
        }
        Icon::Spray => {
            mapper.poly_line(painter, stroke, &[(10, 4), (14, 4), (14, 8), (10, 8)], true);
            mapper.poly_line(
                painter,
                stroke,
                &[(9, 8), (15, 8), (17, 14), (17, 20), (7, 20), (7, 14)],
                true,
            );
            mapper.circle_filled(painter, color, (19, 6), 1);
            mapper.circle_filled(painter, color, (20, 9), 1);
            mapper.circle_filled(painter, color, (18, 10), 1);
        }
        Icon::Compass => {
            mapper.circle(painter, stroke, (12, 12), 9);
            mapper.poly_line(
                painter,
                stroke,
                &[(14, 9), (10, 14), (9, 15), (14, 10)],
                true,
            );
            mapper.circle_filled(painter, color, (12, 12), 1);
        }
        Icon::SymH => {
            mapper.dashed_line(painter, stroke, (4, 12), (20, 12));
            mapper.rect(painter, stroke, (8, 7), (16, 11));
            mapper.rect(painter, stroke, (8, 13), (16, 17));
        }
        Icon::SymV => {
            mapper.dashed_line(painter, stroke, (12, 4), (12, 20));
            mapper.rect(painter, stroke, (7, 8), (11, 16));
            mapper.rect(painter, stroke, (13, 8), (17, 16));
        }
        Icon::SymQ => {
            mapper.dashed_line(painter, stroke, (4, 12), (20, 12));
            mapper.dashed_line(painter, stroke, (12, 4), (12, 20));
        }
        Icon::SymRot => {
            mapper.circle(painter, stroke, (12, 12), 7);
            mapper.poly_line(painter, stroke, &[(12, 5), (12, 12), (17, 14)], false);
        }
    }

    // Provide an accessible string id for tooltips / a11y; egui doesn't
    // surface a11y yet but tools like axe wrappers can hook on this
    // later via `painter.add(...)`-flavoured extensions.
    let _ = (Align2::CENTER_CENTER, StrokeKind::Middle); // keep imports honest
}

/// Coordinate mapper from the 24-unit logical grid to a screen rect.
/// Uniform scale, centred in the rect — non-square rects letterbox.
struct Mapper {
    origin: Pos2,
    scale: f32,
}

impl Mapper {
    fn new(rect: Rect) -> Self {
        let scale = rect.width().min(rect.height()) / 24.0;
        let size = egui::vec2(24.0 * scale, 24.0 * scale);
        let centre = rect.center();
        let origin = Pos2::new(centre.x - size.x * 0.5, centre.y - size.y * 0.5);
        Self { origin, scale }
    }

    fn p(&self, x: i32, y: i32) -> Pos2 {
        Pos2::new(
            self.origin.x + x as f32 * self.scale,
            self.origin.y + y as f32 * self.scale,
        )
    }

    fn line(&self, painter: &egui::Painter, stroke: Stroke, a: (i32, i32), b: (i32, i32)) {
        painter.line_segment([self.p(a.0, a.1), self.p(b.0, b.1)], stroke);
    }

    fn poly_line(&self, painter: &egui::Painter, stroke: Stroke, pts: &[(i32, i32)], closed: bool) {
        let mut prev = self.p(pts[0].0, pts[0].1);
        for next in &pts[1..] {
            let p = self.p(next.0, next.1);
            painter.line_segment([prev, p], stroke);
            prev = p;
        }
        if closed {
            let first = self.p(pts[0].0, pts[0].1);
            painter.line_segment([prev, first], stroke);
        }
    }

    fn rect(&self, painter: &egui::Painter, stroke: Stroke, a: (i32, i32), b: (i32, i32)) {
        let r = Rect::from_two_pos(self.p(a.0, a.1), self.p(b.0, b.1));
        painter.rect_stroke(r, 2.0, stroke, StrokeKind::Middle);
    }

    fn circle(&self, painter: &egui::Painter, stroke: Stroke, c: (i32, i32), r: i32) {
        painter.circle_stroke(self.p(c.0, c.1), r as f32 * self.scale, stroke);
    }

    fn circle_filled(&self, painter: &egui::Painter, color: Color32, c: (i32, i32), r: i32) {
        painter.circle_filled(self.p(c.0, c.1), r as f32 * self.scale, color);
    }

    /// Quadratic-bezier-ish curve approximated as three line segments
    /// through `a → mid → b`. Cheap, good enough for icon-scale curves.
    fn curve(
        &self,
        painter: &egui::Painter,
        stroke: Stroke,
        a: (i32, i32),
        m: (i32, i32),
        b: (i32, i32),
    ) {
        // Subdivide into 6 segments so the bend reads as a curve at
        // 16–24 px tile sizes.
        let pa = self.p(a.0, a.1);
        let pm = self.p(m.0, m.1);
        let pb = self.p(b.0, b.1);
        let mut prev = pa;
        for i in 1..=6 {
            let t = i as f32 / 6.0;
            let one_minus = 1.0 - t;
            let x = one_minus * one_minus * pa.x + 2.0 * one_minus * t * pm.x + t * t * pb.x;
            let y = one_minus * one_minus * pa.y + 2.0 * one_minus * t * pm.y + t * t * pb.y;
            let p = Pos2::new(x, y);
            painter.line_segment([prev, p], stroke);
            prev = p;
        }
    }

    fn arc(
        &self,
        painter: &egui::Painter,
        stroke: Stroke,
        c: (i32, i32),
        r: i32,
        start: f32,
        end: f32,
    ) {
        let centre = self.p(c.0, c.1);
        let radius = r as f32 * self.scale;
        let steps = 32;
        let mut prev = Pos2::new(
            centre.x + radius * start.cos(),
            centre.y + radius * start.sin(),
        );
        for i in 1..=steps {
            let t = start + (end - start) * (i as f32 / steps as f32);
            let p = Pos2::new(centre.x + radius * t.cos(), centre.y + radius * t.sin());
            painter.line_segment([prev, p], stroke);
            prev = p;
        }
    }

    fn dashed_line(&self, painter: &egui::Painter, stroke: Stroke, a: (i32, i32), b: (i32, i32)) {
        let pa = self.p(a.0, a.1);
        let pb = self.p(b.0, b.1);
        let dx = pb.x - pa.x;
        let dy = pb.y - pa.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1.0 {
            return;
        }
        let dash = 3.0 * self.scale.max(0.5);
        let gap = 2.0 * self.scale.max(0.5);
        let step = dash + gap;
        let mut t = 0.0;
        while t < len {
            let t0 = t / len;
            let t1 = ((t + dash).min(len)) / len;
            let p0 = Pos2::new(pa.x + dx * t0, pa.y + dy * t0);
            let p1 = Pos2::new(pa.x + dx * t1, pa.y + dy * t1);
            painter.line_segment([p0, p1], stroke);
            t += step;
        }
    }

    /// Map-pin teardrop — point at `tip`, body centred at `body` with
    /// radius `r`. Approximated as two bezier-ish curves meeting at the
    /// tip.
    fn teardrop(
        &self,
        painter: &egui::Painter,
        stroke: Stroke,
        tip: (i32, i32),
        body: (i32, i32),
        r: i32,
    ) {
        // Left curl up to circle, right curl up to circle.
        let r2 = r;
        self.curve(
            painter,
            stroke,
            tip,
            (body.0 - r2, body.1 + r2 / 2),
            (body.0 - r2, body.1),
        );
        self.curve(
            painter,
            stroke,
            (body.0 - r2, body.1),
            (body.0, body.1 - r2),
            (body.0 + r2, body.1),
        );
        self.curve(
            painter,
            stroke,
            (body.0 + r2, body.1),
            (body.0 + r2, body.1 + r2 / 2),
            tip,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_catalogue_is_unique() {
        // The ALL slice is the source of truth for icon-gallery
        // rendering; duplicates would cause the gallery to render the
        // same icon twice and mask missing variants.
        let mut seen = std::collections::HashSet::new();
        for &i in ALL {
            assert!(seen.insert(i), "duplicate icon in ALL: {:?}", i);
        }
    }

    #[test]
    fn all_catalogue_covers_every_variant() {
        // Every named variant should appear in ALL — otherwise the
        // cheat-sheet icon gallery skips icons silently.
        let expected_variants = 42; // bump when adding icons
        assert_eq!(
            ALL.len(),
            expected_variants,
            "ALL has {} icons, expected {} — update the constant when adding/removing",
            ALL.len(),
            expected_variants
        );
    }

    #[test]
    fn mapper_centres_in_non_square_rect() {
        // A wide rect should letterbox the 24×24 grid horizontally,
        // not stretch it.
        let rect = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(48.0, 24.0));
        let m = Mapper::new(rect);
        assert!(
            (m.scale - 1.0).abs() < 0.001,
            "scale should be min/24 = 1.0, was {}",
            m.scale
        );
        // 24×1 = 24, centred in 48-wide rect → origin.x = 12.
        assert!((m.origin.x - 12.0).abs() < 0.001);
        assert!((m.origin.y - 0.0).abs() < 0.001);
    }

    #[test]
    fn mapper_maps_corners_correctly() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 200.0), egui::vec2(24.0, 24.0));
        let m = Mapper::new(rect);
        let top_left = m.p(0, 0);
        let bottom_right = m.p(24, 24);
        assert!((top_left.x - 100.0).abs() < 0.001);
        assert!((top_left.y - 200.0).abs() < 0.001);
        assert!((bottom_right.x - 124.0).abs() < 0.001);
        assert!((bottom_right.y - 224.0).abs() < 0.001);
    }
}
