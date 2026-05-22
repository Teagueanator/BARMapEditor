//! Sprint 31 / U4 — confirmation modal primitive.
//!
//! Replaces the pre-Sprint-31 mix of direct destructive actions
//! (delete ally group silently destroys its start positions,
//! delete layer silently destroys mask data, new-project /
//! open-project discards unsaved edits) and bespoke
//! `egui::Window` modals (Sprint 17's migration toast,
//! Sprint 20's save-before-build stub).
//!
//! ## Render contract
//!
//! - **Backdrop**: a fullscreen rectangle on `egui::Order::Foreground`
//!   minus one layer, painted as ~50 % black to dim the editor
//!   behind. Clicks on the backdrop are absorbed (no click-through
//!   onto the underlying viewport / panels) — per PITFALL #4 + #11.
//! - **Dialog**: centred Window at `Order::Foreground`, fixed
//!   width 360 px, neutral border with the `t.danger` red border
//!   reserved for the confirm button when `destructive`. Esc =
//!   cancel; Enter = confirm. Other keys pass through.
//! - **Lifecycle**: the App holds `Option<ConfirmDialog>` and
//!   sets it `Some` on the destructive code path; the next
//!   frame's [`confirm_modal`] returns `Some(result)` once the
//!   user clicks Confirm or Cancel (or presses Enter / Esc) and
//!   the App applies the resolution by `take`-ing the option.
//!
//! ## Critical pitfalls observed
//!
//! - PITFALL #4 (keyboard scope): only Enter + Esc are
//!   consumed; the rest of the keyboard surface remains
//!   available so a user typing into a focused widget elsewhere
//!   isn't blocked by an open modal (defensive — the backdrop
//!   absorbs clicks, so focus practically can't reach a
//!   non-modal widget while the dialog is up, but the input
//!   plumbing belt-and-braces this).
//! - PITFALL #5 (danger-button colour): when `destructive`, the
//!   confirm button paints `t.red` background + white text;
//!   non-destructive variants use the default accent.
//! - PITFALL #8 (modal != undo): the modal's lifecycle does
//!   NOT touch the undo history; the action it gates does (via
//!   `ProjectDiff`). Cancelling the modal is a pure no-op on
//!   the project state.
//! - PITFALL #11 (egui has no true modal): we emulate one with
//!   a fullscreen backdrop area at `Foreground` order that
//!   intercepts every click, plus a dialog Window above it.

use eframe::egui::{self, Color32, CornerRadius, Sense, Stroke, StrokeKind};
use tracing::trace;

use crate::ui::theme::Tokens;

/// One-shot dialog descriptor. The App writes one of these into
/// `App::pending_confirm` when a destructive path needs gating;
/// [`confirm_modal`] renders it on subsequent frames until the
/// user resolves it.
#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    pub title: &'static str,
    pub message: String,
    pub confirm_label: &'static str,
    pub cancel_label: &'static str,
    /// When `true`, the confirm button paints with the danger red
    /// background + white foreground so the user sees the gravity
    /// at a glance.
    pub destructive: bool,
}

impl ConfirmDialog {
    /// Convenience constructor for a destructive prompt
    /// ("Delete", "Discard") — `confirm_label` defaults to the
    /// verb the caller passed; `cancel_label` is "Cancel".
    pub fn destructive(
        title: &'static str,
        message: impl Into<String>,
        confirm_label: &'static str,
    ) -> Self {
        Self {
            title,
            message: message.into(),
            confirm_label,
            cancel_label: "Cancel",
            destructive: true,
        }
    }
}

/// Resolution returned by [`confirm_modal`] once the user
/// clicks a button or presses Enter / Esc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResult {
    Confirmed,
    Cancelled,
}

/// Render the confirmation modal if `state` is `Some`. Returns
/// `Some(result)` on the resolving frame and clears `state` back
/// to `None`; subsequent frames re-render the editor underneath
/// without the dialog.
///
/// The signature takes `&mut Option<ConfirmDialog>` so the
/// `take()` happens inside this function — the caller never has
/// to think about ordering the clear before the next read.
pub fn confirm_modal(
    ctx: &egui::Context,
    state: &mut Option<ConfirmDialog>,
) -> Option<ConfirmResult> {
    let dialog = state.as_ref()?;
    let t = Tokens::DARK;
    let viewport = ctx.content_rect();

    // ─── Backdrop ───────────────────────────────────────────────
    // Order::Background-of-Foreground is approximated by drawing
    // a fullscreen click-eating Area underneath the dialog
    // Window. We can't use Order::Background because that sits
    // below CentralPanel; we need above-everything-but-the-dialog.
    let backdrop_id = egui::Id::new("confirm_modal_backdrop");
    egui::Area::new(backdrop_id)
        .order(egui::Order::Foreground)
        .fixed_pos(viewport.min)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, _resp) = ui.allocate_exact_size(viewport.size(), Sense::click());
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(0),
                Color32::from_rgba_premultiplied(0, 0, 0, 0x90),
            );
        });

    // ─── Dialog Window ──────────────────────────────────────────
    let title = dialog.title;
    let message = dialog.message.clone();
    let confirm_label = dialog.confirm_label;
    let cancel_label = dialog.cancel_label;
    let destructive = dialog.destructive;

    let mut outcome: Option<ConfirmResult> = None;
    let dialog_id = egui::Id::new(("confirm_modal_window", title));
    egui::Area::new(dialog_id)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .interactable(true)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(t.panel)
                .stroke(Stroke::new(1.0, t.border_hi))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::same(16))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 12,
                    spread: 0,
                    color: Color32::from_rgba_premultiplied(0, 0, 0, 0x60),
                });
            frame.show(ui, |ui| {
                ui.set_min_width(360.0);
                ui.set_max_width(360.0);
                ui.label(egui::RichText::new(title).strong().size(14.0).color(t.text));
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&message).color(t.muted).size(12.0));
                ui.add_space(14.0);
                // Right-align the buttons: Cancel on the left,
                // Confirm on the right (Photoshop / GNOME
                // convention — destructive primary action sits at
                // the trailing edge so muscle memory finds it).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let confirm_resp = ui.add(confirm_button(confirm_label, destructive, &t));
                    if confirm_resp.clicked() {
                        outcome = Some(ConfirmResult::Confirmed);
                    }
                    ui.add_space(6.0);
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new(cancel_label).size(12.0),
                        ))
                        .clicked()
                    {
                        outcome = Some(ConfirmResult::Cancelled);
                    }
                });
            });
        });

    // ─── Keyboard (Enter = confirm, Esc = cancel) ──────────────
    // Consume the events so they don't reach other widgets.
    if outcome.is_none() {
        ctx.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                outcome = Some(ConfirmResult::Confirmed);
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                outcome = Some(ConfirmResult::Cancelled);
            }
        });
    }

    if let Some(result) = outcome {
        trace!(
            target: "barme::confirm",
            title,
            ?result,
            destructive,
            "confirm_modal resolved"
        );
        *state = None;
        return Some(result);
    }
    None
}

/// Helper widget — paint the confirm button. Destructive variants
/// render with `t.red` background + white text; the rest use the
/// stock accent.
fn confirm_button<'a>(label: &'a str, destructive: bool, t: &'a Tokens) -> impl egui::Widget + 'a {
    move |ui: &mut egui::Ui| {
        let galley = ui.painter().layout_no_wrap(
            label.to_string(),
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
        let pad_x = 12.0;
        let pad_y = 6.0;
        let size = galley.size() + egui::vec2(pad_x * 2.0, pad_y * 2.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let bg = if destructive { t.red } else { t.accent };
        let hovered_bg = if response.hovered() {
            // 90 % opacity to give the hover a subtle lift.
            Color32::from_rgba_premultiplied(
                ((bg.r() as u16) * 230 / 255) as u8,
                ((bg.g() as u16) * 230 / 255) as u8,
                ((bg.b() as u16) * 230 / 255) as u8,
                255,
            )
        } else {
            bg
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(4), hovered_bg);
        painter.rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(1.0, bg),
            StrokeKind::Middle,
        );
        let text_pos =
            egui::Pos2::new(rect.left() + pad_x, rect.center().y - galley.size().y * 0.5);
        painter.galley(text_pos, galley, Color32::WHITE);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_state_returns_none() {
        let ctx = egui::Context::default();
        let mut state: Option<ConfirmDialog> = None;
        let mut result: Option<ConfirmResult> = Some(ConfirmResult::Confirmed);
        let _ = ctx.run(Default::default(), |ctx| {
            result = confirm_modal(ctx, &mut state);
        });
        assert!(result.is_none());
        assert!(state.is_none());
    }

    #[test]
    fn open_dialog_stays_open_without_input() {
        let ctx = egui::Context::default();
        let mut state = Some(ConfirmDialog::destructive(
            "Delete",
            "Really delete?",
            "Delete",
        ));
        let mut result: Option<ConfirmResult> = None;
        let _ = ctx.run(Default::default(), |ctx| {
            result = confirm_modal(ctx, &mut state);
        });
        assert!(result.is_none());
        assert!(state.is_some(), "dialog should persist without input");
    }

    #[test]
    fn escape_key_cancels() {
        let ctx = egui::Context::default();
        let mut state = Some(ConfirmDialog::destructive(
            "Delete",
            "Really delete?",
            "Delete",
        ));
        let mut result: Option<ConfirmResult> = None;
        // Fabricate a frame with an Escape key event in the input.
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(raw, |ctx| {
            result = confirm_modal(ctx, &mut state);
        });
        assert_eq!(result, Some(ConfirmResult::Cancelled));
        assert!(state.is_none(), "resolution should clear state");
    }

    #[test]
    fn enter_key_confirms() {
        let ctx = egui::Context::default();
        let mut state = Some(ConfirmDialog::destructive(
            "Delete",
            "Really delete?",
            "Delete",
        ));
        let mut result: Option<ConfirmResult> = None;
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(raw, |ctx| {
            result = confirm_modal(ctx, &mut state);
        });
        assert_eq!(result, Some(ConfirmResult::Confirmed));
        assert!(state.is_none());
    }

    #[test]
    fn destructive_constructor_sets_flag() {
        let d = ConfirmDialog::destructive("X", "body", "Delete");
        assert!(d.destructive);
        assert_eq!(d.cancel_label, "Cancel");
        assert_eq!(d.confirm_label, "Delete");
    }

    /// A non-Enter / non-Esc keypress must NOT resolve the dialog.
    /// This pins the keyboard scope (PITFALL #4): only the two
    /// gating keys consume input; other typing passes through.
    #[test]
    fn other_keys_do_not_resolve() {
        let ctx = egui::Context::default();
        let mut state = Some(ConfirmDialog::destructive("X", "body", "Delete"));
        let mut result: Option<ConfirmResult> = None;
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let _ = ctx.run(raw, |ctx| {
            result = confirm_modal(ctx, &mut state);
        });
        assert!(result.is_none(), "A key must not resolve the dialog");
        assert!(state.is_some(), "dialog should still be open");
    }
}
