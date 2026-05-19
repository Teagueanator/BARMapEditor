//! Visual identity for the editor (ADR-035).
//!
//! Owns the single source of truth for the colour palette, font sizes,
//! and corner radii used across the top action bar, left tool strip,
//! right inspector, status strip, and viewport chrome. Every panel
//! reads from [`Tokens`] rather than hard-coding `Color32` constants
//! inline, so a future theme variant (high-contrast, day mode) only has
//! to swap one constructor.
//!
//! Palette mirrors the Claude Design mockup at
//! `/tmp/bar-ui-design/editor-shell.jsx:6` — see `docs/research/ui/`
//! for the design rationale. Names match the mockup tokens 1:1 to
//! preserve grep-ability between the JSX reference and the Rust
//! implementation.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, Visuals};

/// Editor palette + spacing tokens. All values are `const` so they can
/// be folded by the compiler; the indirection exists for readability,
/// not runtime swapping.
#[derive(Clone, Copy)]
pub struct Tokens {
    // backgrounds
    pub bg: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub hover: Color32,

    // borders
    pub border: Color32,
    pub border_hi: Color32,

    // text
    pub text: Color32,
    pub muted: Color32,
    pub dim: Color32,

    // accents
    pub accent: Color32,
    /// Soft translucent variant of `accent`. Reserved for selected-card
    /// backgrounds in future ADRs (today the accent shade is computed
    /// via [`Self::accent_alpha`] on demand).
    #[allow(dead_code)]
    pub accent_soft: Color32,
    pub accent_dim: Color32,

    // semantic
    pub green: Color32,
    pub amber: Color32,
    pub red: Color32,
}

impl Tokens {
    /// The dark DCC palette. Mirrors the JSX mockup; do not edit a
    /// single value without re-checking the mockup screenshots.
    pub const DARK: Tokens = Tokens {
        bg: Color32::from_rgb(0x1B, 0x1B, 0x1F),
        panel: Color32::from_rgb(0x26, 0x26, 0x2C),
        panel2: Color32::from_rgb(0x2A, 0x2A, 0x31),
        hover: Color32::from_rgb(0x33, 0x33, 0x3B),

        border: Color32::from_rgb(0x3A, 0x3A, 0x42),
        border_hi: Color32::from_rgb(0x4A, 0x4A, 0x54),

        text: Color32::from_rgb(0xE5, 0xE5, 0xEA),
        muted: Color32::from_rgb(0x9C, 0xA3, 0xAF),
        dim: Color32::from_rgb(0x6B, 0x72, 0x80),

        accent: Color32::from_rgb(0x3B, 0x82, 0xF6),
        // `accent_soft` and `accent_dim` are premultiplied translucent
        // forms of the accent for fills/borders that need to read at
        // ~18 % / ~50 % opacity against the panel bg.
        accent_soft: Color32::from_rgba_premultiplied(0x0B, 0x18, 0x2C, 0x2E),
        accent_dim: Color32::from_rgba_premultiplied(0x1E, 0x41, 0x7B, 0x80),

        green: Color32::from_rgb(0x22, 0xC5, 0x5E),
        amber: Color32::from_rgb(0xF5, 0x9E, 0x0B),
        red: Color32::from_rgb(0xEF, 0x44, 0x44),
    };

    /// Premultiplied translucent variant of [`accent`] at the given
    /// alpha (0..=255). Used for selected-card backgrounds where the
    /// card sits on the panel and needs to read as accent without
    /// dominating.
    pub fn accent_alpha(self, alpha: u8) -> Color32 {
        let a = alpha as u16;
        let scale = |c: u8| (((c as u16) * a) / 255) as u8;
        Color32::from_rgba_premultiplied(
            scale(self.accent.r()),
            scale(self.accent.g()),
            scale(self.accent.b()),
            alpha,
        )
    }

    /// Tone-coloured background for [`crate::ui::widgets::chip`]. Each
    /// tone returns a translucent fill that matches the mockup's
    /// `rgba(<tone>, .10)` chip background.
    pub fn chip_bg(self, tone: ChipTone) -> Color32 {
        match tone {
            ChipTone::Neutral => Color32::TRANSPARENT,
            ChipTone::Ok => Color32::from_rgba_premultiplied(0x03, 0x14, 0x09, 0x1A),
            ChipTone::Warn => Color32::from_rgba_premultiplied(0x18, 0x10, 0x01, 0x1A),
            ChipTone::Err => Color32::from_rgba_premultiplied(0x18, 0x07, 0x07, 0x1A),
        }
    }

    /// Foreground colour for a [`ChipTone`].
    pub fn chip_fg(self, tone: ChipTone) -> Color32 {
        match tone {
            ChipTone::Neutral => self.muted,
            ChipTone::Ok => self.green,
            ChipTone::Warn => self.amber,
            ChipTone::Err => self.red,
        }
    }
}

/// Semantic tone for chips, status dots, and validation badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipTone {
    Neutral,
    Ok,
    Warn,
    Err,
}

/// Install the dark theme on the given context. Idempotent — calling
/// again replaces the visuals wholesale.
pub fn install(ctx: &egui::Context) {
    install_visuals(ctx, Tokens::DARK);
    install_style(ctx);
}

/// Build an [`egui::Visuals`] from a palette and apply it. Exposed
/// separately so tests can construct visuals without spinning up a
/// full `egui::Context`.
pub fn install_visuals(ctx: &egui::Context, t: Tokens) {
    let mut v = Visuals::dark();
    v.window_fill = t.panel;
    v.panel_fill = t.panel;
    v.faint_bg_color = t.panel2;
    v.extreme_bg_color = t.bg;
    v.code_bg_color = t.bg;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.bg_fill = t.panel;
    v.widgets.noninteractive.weak_bg_fill = t.panel2;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.bg_fill = t.panel2;
    v.widgets.inactive.weak_bg_fill = t.panel2;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.border_hi);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.active.bg_fill = t.accent;
    v.widgets.active.weak_bg_fill = t.accent;
    v.widgets.active.bg_stroke = Stroke::new(1.0, t.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.open.bg_fill = t.hover;
    v.widgets.open.weak_bg_fill = t.panel2;
    v.widgets.open.bg_stroke = Stroke::new(1.0, t.border_hi);
    v.selection.bg_fill = t.accent_dim;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.override_text_color = Some(t.text);
    v.hyperlink_color = t.accent;
    ctx.set_visuals(v);
}

/// Tune spacing, font sizes, and corner radii to the mockup. Separated
/// from `install_visuals` so tests can assert visuals in isolation.
pub fn install_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    // The mockup uses 4 px corner radii almost everywhere; the only
    // exceptions are chips (10 px pill) and modals (8 px). Those are
    // applied locally where they're drawn.
    let r = CornerRadius::same(4);
    style.visuals.widgets.noninteractive.corner_radius = r;
    style.visuals.widgets.inactive.corner_radius = r;
    style.visuals.widgets.hovered.corner_radius = r;
    style.visuals.widgets.active.corner_radius = r;
    style.visuals.widgets.open.corner_radius = r;
    style.visuals.window_corner_radius = CornerRadius::same(8);
    style.visuals.menu_corner_radius = CornerRadius::same(6);

    // Typography hierarchy. Sizes mirror the mockup — small UI chips at
    // 10/11 px, body at 12 px, prominent labels at 13 px, modal headers
    // at 15 px. Egui doesn't render emoji at custom sizes well, so we
    // keep the Body font slot at 13 to read with the proportional
    // default; lower sizes are reached via local FontId overrides.
    style.text_styles.insert(
        egui::TextStyle::Heading,
        FontId::new(15.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Monospace,
        FontId::new(11.0, FontFamily::Monospace),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        FontId::new(10.0, FontFamily::Proportional),
    );

    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.menu_margin = egui::Margin::same(4);
    style.spacing.window_margin = egui::Margin::same(8);

    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_palette_has_distinct_text_colours() {
        let t = Tokens::DARK;
        // The four text tones must be distinguishable; if any pair
        // collapses, the typography hierarchy in the mockup is lost.
        assert_ne!(t.text, t.muted);
        assert_ne!(t.muted, t.dim);
        assert_ne!(t.text, t.dim);
    }

    #[test]
    fn dark_palette_has_dark_backgrounds() {
        let t = Tokens::DARK;
        // Every background must be darker than the text colour, or the
        // theme would be unreadable against text-on-bg compositions.
        let luma = |c: Color32| {
            // Rough luminance — enough to assert the relationship.
            (c.r() as u32 + c.g() as u32 + c.b() as u32) / 3
        };
        assert!(luma(t.bg) < luma(t.text));
        assert!(luma(t.panel) < luma(t.text));
        assert!(luma(t.panel2) < luma(t.text));
        assert!(luma(t.hover) < luma(t.text));
    }

    #[test]
    fn accent_alpha_scales_premultiplied() {
        // accent_alpha(255) should round-trip the opaque accent (the
        // premultiplied formula multiplies channels by a/255).
        let t = Tokens::DARK;
        let full = t.accent_alpha(255);
        assert_eq!(full.a(), 255);
        assert_eq!(full.r(), t.accent.r());
        assert_eq!(full.g(), t.accent.g());
        assert_eq!(full.b(), t.accent.b());

        // accent_alpha(0) yields a fully transparent black.
        let zero = t.accent_alpha(0);
        assert_eq!(zero.a(), 0);
        assert_eq!(zero.r(), 0);
        assert_eq!(zero.g(), 0);
        assert_eq!(zero.b(), 0);
    }

    #[test]
    fn chip_tones_all_distinct() {
        let t = Tokens::DARK;
        let tones = [
            ChipTone::Neutral,
            ChipTone::Ok,
            ChipTone::Warn,
            ChipTone::Err,
        ];
        let fgs: Vec<Color32> = tones.iter().copied().map(|x| t.chip_fg(x)).collect();
        // Each tone's foreground must be unique so a user reading the
        // chip can distinguish success/warning/error/neutral by colour
        // alone.
        for i in 0..fgs.len() {
            for j in (i + 1)..fgs.len() {
                assert_ne!(
                    fgs[i], fgs[j],
                    "chip fg collision: {:?} vs {:?}",
                    tones[i], tones[j]
                );
            }
        }
    }
}
