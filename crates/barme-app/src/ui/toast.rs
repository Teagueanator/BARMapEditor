//! Sprint 31 / U4 — toast queue primitive.
//!
//! Replaces the pre-Sprint-31 `App::last_error: Option<String>`
//! slot in the status strip, which had three problems:
//!
//! 1. Single-slot — the latest message overwrites everything
//!    before it. A texture-GC report would erase a "save failed"
//!    error before the user saw it.
//! 2. No tone distinction — info, warning, and error all read as
//!    the same red label.
//! 3. No action affordance — the user couldn't click to open the
//!    relevant panel (build log, lint panel, help center).
//!
//! ## Render contract
//!
//! Toasts stack from the **bottom-right** of the viewport upward,
//! anchored via `egui::Area` so panels don't reshuffle when one is
//! shown. The most recent toast sits at the bottom (closest to the
//! status strip) so the eye doesn't jump to the top of the
//! viewport for the latest event.
//!
//! ## Auto-dismiss timing
//!
//! - `Info`: 3 seconds — lifecycle confirmations (save ok, GC swept).
//! - `Warning`: 6 seconds — non-fatal degradation (downsample,
//!   lint count update).
//! - `Error`: persistent until the user clicks the × dismiss button
//!   (PITFALL #9 — never auto-drop an error).
//!
//! ## Rate-limit + cap
//!
//! Duplicate text within 5 seconds collapses into `"<text> (×N)"`
//! on the existing toast — preventing toast-spam when a worker
//! re-emits the same warning every frame (PITFALL #2 of this
//! sprint's brief). The queue is hard-capped at 10 active toasts;
//! beyond that the oldest non-error is dropped first.
//!
//! ## Critical pitfalls observed
//!
//! - PITFALL #3 (10-cap): enforced in [`ToastQueue::spawn`].
//! - PITFALL #6 (action button is the only path to the build
//!   log): the toast body's hover-click does NOT open the log;
//!   only the explicit `[View log]` button does.
//! - PITFALL #9 (errors must be dismissable): the × renders on
//!   every toast regardless of `dismiss_at`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke};
use tracing::trace;

use crate::ui::help_center::HelpArticleId;
use crate::ui::theme::Tokens;

/// Maximum number of toasts visible at once. Beyond this, the
/// queue drops the oldest **non-error** entry; errors are pinned.
pub const MAX_TOASTS: usize = 10;

/// Rate-limit window for the "(×N)" coalescing. Duplicate text
/// arriving within this window of an existing toast (with the
/// same kind) updates the existing toast's counter instead of
/// spawning a new one.
pub const DEDUP_WINDOW: Duration = Duration::from_secs(5);

/// Auto-dismiss for [`ToastKind::Info`]. Lifecycle confirmations.
pub const INFO_TTL: Duration = Duration::from_secs(3);

/// Auto-dismiss for [`ToastKind::Warning`]. Degradation notes.
pub const WARN_TTL: Duration = Duration::from_secs(6);

/// Fade-out animation duration in the trailing window before
/// dismissal. Errors are persistent so they never fade.
pub const FADE_OUT: Duration = Duration::from_millis(500);

/// Semantic tone for a toast — drives the leading icon colour
/// and the auto-dismiss timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warning,
    Error,
}

/// Side-effect a toast can request when its action button is
/// clicked. The App's update loop maps each variant to a
/// concrete state mutation; the toast module stays free of
/// `&mut App`.
///
/// `OpenHelpArticle` is only wired by chunk 5's
/// surface-system-events pass; the other variants land in
/// chunks 2 + 4.
#[allow(dead_code)] // OpenHelpArticle lands in chunk 5
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastAction {
    /// Open the Sprint-19 lint panel. Used by the "N issues" toast
    /// the validation-chip click surfaces.
    OpenLintPanel,
    /// Open the Sprint-20 build log panel. Used by the "Build
    /// failed" / "Build cancelled" toasts.
    OpenBuildLog,
    /// Open the Sprint-22 help center, jumping to the named
    /// article. Used by toasts that reference a known pitfall.
    OpenHelpArticle(HelpArticleId),
    /// Sprint-17 migration: dismiss + persist the
    /// `Project.migration_toast_dismissed` flag so the toast
    /// doesn't reappear on subsequent opens.
    DismissMigrationToast,
}

impl ToastAction {
    /// Button label rendered next to the toast text.
    pub fn label(&self) -> &'static str {
        match self {
            ToastAction::OpenLintPanel => "Open lint panel",
            ToastAction::OpenBuildLog => "View log",
            ToastAction::OpenHelpArticle(_) => "Learn more",
            ToastAction::DismissMigrationToast => "Don't show again",
        }
    }
}

/// A single notification.
#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub spawned_at: Instant,
    /// `None` = persistent (must be dismissed by the user).
    pub dismiss_at: Option<Instant>,
    /// Optional action button that fires a [`ToastAction`] when
    /// clicked.
    pub action: Option<ToastAction>,
    /// Duplicate-coalesce counter. Starts at 1; bumps when an
    /// identical-text toast arrives within [`DEDUP_WINDOW`].
    pub count: u32,
}

impl Toast {
    /// Helper for tests + the queue's auto-dismiss check: has
    /// the toast's `dismiss_at` passed?
    pub fn is_expired(&self, now: Instant) -> bool {
        match self.dismiss_at {
            Some(t) => now >= t,
            None => false,
        }
    }

    /// 0..1 fade-out alpha for the trailing [`FADE_OUT`] ms before
    /// dismissal. Errors never fade (their `dismiss_at` is `None`).
    pub fn fade_alpha(&self, now: Instant) -> f32 {
        let Some(dismiss_at) = self.dismiss_at else {
            return 1.0;
        };
        let remaining = dismiss_at.saturating_duration_since(now);
        if remaining >= FADE_OUT {
            1.0
        } else {
            remaining.as_secs_f32() / FADE_OUT.as_secs_f32()
        }
    }

    /// Default auto-dismiss timing for `kind`. `Info`/`Warning`
    /// auto-expire; `Error` is persistent.
    pub fn default_ttl(kind: ToastKind) -> Option<Duration> {
        match kind {
            ToastKind::Info => Some(INFO_TTL),
            ToastKind::Warning => Some(WARN_TTL),
            ToastKind::Error => None,
        }
    }
}

/// The toast queue lives on `App`. Push via the `App::toast_*`
/// helpers; the render loop calls [`render`] each frame to draw
/// and auto-prune.
#[derive(Debug, Default, Clone)]
pub struct ToastQueue {
    pub toasts: VecDeque<Toast>,
    /// Sprint-31 telemetry — total toasts spawned since process
    /// start. Surfaces in trace logs; not user-visible.
    pub total_spawned: u64,
}

impl ToastQueue {
    /// Spawn a new toast OR coalesce into an existing duplicate.
    /// Returns `true` if a fresh toast was added, `false` if the
    /// call merged into an existing entry.
    pub fn spawn(&mut self, kind: ToastKind, text: String, action: Option<ToastAction>) -> bool {
        let now = Instant::now();
        // PITFALL #2 — coalesce duplicates within DEDUP_WINDOW.
        // The dedupe key is (kind, text); action is treated as
        // metadata that can update on the coalesce.
        for existing in self.toasts.iter_mut() {
            if existing.kind == kind
                && existing.text == text
                && now.saturating_duration_since(existing.spawned_at) <= DEDUP_WINDOW
            {
                existing.count = existing.count.saturating_add(1);
                existing.spawned_at = now;
                existing.dismiss_at = Toast::default_ttl(kind).map(|d| now + d);
                if action.is_some() {
                    existing.action = action;
                }
                trace!(
                    target: "barme::toast",
                    ?kind,
                    count = existing.count,
                    "toast coalesced"
                );
                return false;
            }
        }
        let dismiss_at = Toast::default_ttl(kind).map(|d| now + d);
        let toast = Toast {
            kind,
            text,
            spawned_at: now,
            dismiss_at,
            action,
            count: 1,
        };
        self.toasts.push_back(toast);
        self.total_spawned = self.total_spawned.saturating_add(1);
        trace!(
            target: "barme::toast",
            ?kind,
            queue_len = self.toasts.len(),
            "toast spawned"
        );
        // PITFALL #3 — hard-cap at MAX_TOASTS. Drop the oldest
        // non-error first; if every entry is an error, fall back
        // to dropping the absolute oldest (errors get clobbered
        // only when the user is being notification-bombed past
        // any sensible cap).
        while self.toasts.len() > MAX_TOASTS {
            let drop_idx = self
                .toasts
                .iter()
                .position(|t| !matches!(t.kind, ToastKind::Error))
                .unwrap_or(0);
            self.toasts.remove(drop_idx);
        }
        true
    }

    /// Prune expired toasts (auto-dismiss). Called from
    /// [`render`] each frame before painting.
    pub fn prune_expired(&mut self, now: Instant) {
        self.toasts.retain(|t| !t.is_expired(now));
    }

    /// Remove a toast at `index`. Used by the × dismiss button.
    pub fn dismiss(&mut self, index: usize) {
        if index < self.toasts.len() {
            let removed = self.toasts.remove(index);
            if let Some(t) = removed {
                trace!(
                    target: "barme::toast",
                    kind = ?t.kind,
                    "toast dismissed by user"
                );
            }
        }
    }

    /// Count of active toasts in each tone bucket. Drives the
    /// status-strip chip.
    #[allow(dead_code)] // wired by chunk 2 (status-strip count chip)
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut info = 0;
        let mut warn = 0;
        let mut err = 0;
        for t in &self.toasts {
            match t.kind {
                ToastKind::Info => info += 1,
                ToastKind::Warning => warn += 1,
                ToastKind::Error => err += 1,
            }
        }
        (info, warn, err)
    }
}

/// Per-frame interaction outcome. The render call returns a
/// [`Vec<ToastInteraction>`] so the App's update loop can react
/// without `&mut App` reaching into the toast module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastInteraction {
    /// User clicked the × — remove the toast at the given index.
    Dismiss(usize),
    /// User clicked the action button — fire the action, then
    /// remove the toast.
    Action(usize, ToastAction),
}

/// Render every active toast. Returns the set of interactions
/// this frame; the caller (the App's update loop) applies them
/// in order.
///
/// The render uses `egui::Area::order(Foreground)` so the toast
/// stack floats over panels but is non-blocking — clicks outside
/// the toast bodies pass through to the underlying widgets.
pub fn render(
    ctx: &egui::Context,
    queue: &ToastQueue,
    viewport_rect: Rect,
) -> Vec<ToastInteraction> {
    let now = Instant::now();
    let mut interactions = Vec::new();
    if queue.toasts.is_empty() {
        return interactions;
    }
    let t = Tokens::DARK;
    let toast_w = 320.0;
    let margin = 12.0;
    // Anchor bottom-right. The bottommost toast is the most
    // recent (highest index in the VecDeque). We stack upward
    // by rendering each toast at a y offset.
    let mut y_cursor = viewport_rect.bottom() - margin;
    // Render in reverse so newest sits at the bottom.
    for (idx, toast) in queue.toasts.iter().enumerate().rev() {
        let alpha = toast.fade_alpha(now);
        let area_id = egui::Id::new(("toast_area", idx));
        let area_resp = egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .fixed_pos(Pos2::new(
                viewport_rect.right() - toast_w - margin,
                y_cursor - 64.0,
            ))
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(toast_w);
                let outcome = paint_toast(ui, toast, alpha, &t);
                if let Some(out) = outcome {
                    match out {
                        ToastOutcome::Dismiss => interactions.push(ToastInteraction::Dismiss(idx)),
                        ToastOutcome::Action => {
                            if let Some(action) = toast.action.clone() {
                                interactions.push(ToastInteraction::Action(idx, action));
                            }
                        }
                    }
                }
            });
        let used_h = area_resp.response.rect.height();
        y_cursor -= used_h + 6.0;
    }
    interactions
}

#[derive(Debug, Clone, Copy)]
enum ToastOutcome {
    Dismiss,
    Action,
}

/// Paint a single toast body. Returns the click outcome (if any).
fn paint_toast(ui: &mut egui::Ui, toast: &Toast, alpha: f32, t: &Tokens) -> Option<ToastOutcome> {
    // Tone-driven palette.
    let (tone_color, tone_label) = match toast.kind {
        ToastKind::Info => (t.accent, "INFO"),
        ToastKind::Warning => (t.amber, "WARN"),
        ToastKind::Error => (t.red, "ERROR"),
    };
    let bg_alpha = ((220.0 * alpha) as u8).max(40);
    let bg = Color32::from_rgba_premultiplied(
        ((t.panel.r() as u16) * (bg_alpha as u16) / 255) as u8,
        ((t.panel.g() as u16) * (bg_alpha as u16) / 255) as u8,
        ((t.panel.b() as u16) * (bg_alpha as u16) / 255) as u8,
        bg_alpha,
    );
    let mut outcome = None;
    let frame = egui::Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, tone_color))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin {
            left: 10,
            right: 10,
            top: 8,
            bottom: 8,
        });
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            // Tone marker dot (8 px) + tone label.
            let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), Sense::hover());
            ui.painter()
                .rect_filled(dot_rect.expand(0.0), CornerRadius::same(4), tone_color);
            ui.label(
                egui::RichText::new(tone_label)
                    .size(9.0)
                    .strong()
                    .color(tone_color),
            );
            // Right-aligned × button.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::click());
                let color = if resp.hovered() { t.text } else { t.muted };
                let painter = ui.painter();
                if resp.hovered() {
                    painter.rect_filled(rect, CornerRadius::same(3), t.hover);
                }
                crate::ui::icons::paint_icon(
                    painter,
                    rect.shrink(2.0),
                    crate::ui::icons::Icon::X,
                    color,
                    1.5,
                );
                if resp.on_hover_text("Dismiss this notification.").clicked() {
                    outcome = Some(ToastOutcome::Dismiss);
                }
            });
        });
        ui.add_space(2.0);
        // Body — text + (×N) counter when coalesced.
        let body = if toast.count > 1 {
            format!("{} (×{})", toast.text, toast.count)
        } else {
            toast.text.clone()
        };
        ui.label(egui::RichText::new(body).color(t.text).size(11.5));
        // Action button row (if any).
        if let Some(action) = &toast.action {
            ui.add_space(6.0);
            if ui
                .add(egui::Button::new(
                    egui::RichText::new(action.label()).size(11.0),
                ))
                .clicked()
            {
                outcome = Some(ToastOutcome::Action);
            }
        }
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_appends_to_queue() {
        let mut q = ToastQueue::default();
        assert!(q.spawn(ToastKind::Info, "hello".into(), None));
        assert_eq!(q.toasts.len(), 1);
        assert_eq!(q.toasts[0].kind, ToastKind::Info);
        assert_eq!(q.toasts[0].count, 1);
    }

    #[test]
    fn duplicate_text_within_window_coalesces() {
        let mut q = ToastQueue::default();
        assert!(q.spawn(ToastKind::Warning, "same".into(), None));
        assert!(!q.spawn(ToastKind::Warning, "same".into(), None));
        assert!(!q.spawn(ToastKind::Warning, "same".into(), None));
        assert_eq!(q.toasts.len(), 1);
        assert_eq!(q.toasts[0].count, 3);
    }

    #[test]
    fn different_kinds_do_not_coalesce() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Info, "x".into(), None);
        q.spawn(ToastKind::Warning, "x".into(), None);
        assert_eq!(q.toasts.len(), 2);
    }

    #[test]
    fn cap_drops_oldest_non_error() {
        let mut q = ToastQueue::default();
        // Spawn 12 (above the cap). The first one is an error
        // that must survive.
        q.spawn(ToastKind::Error, "boom".into(), None);
        for i in 0..11 {
            q.spawn(ToastKind::Info, format!("info {i}"), None);
        }
        assert_eq!(q.toasts.len(), MAX_TOASTS);
        // Error must still be present.
        assert!(q.toasts.iter().any(|t| t.text == "boom"));
    }

    #[test]
    fn error_has_no_auto_dismiss() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Error, "fatal".into(), None);
        let now = Instant::now() + Duration::from_secs(3600);
        q.prune_expired(now);
        assert_eq!(q.toasts.len(), 1, "errors must persist across prune");
    }

    #[test]
    fn info_auto_dismisses_after_ttl() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Info, "ok".into(), None);
        let future = Instant::now() + INFO_TTL + Duration::from_millis(10);
        q.prune_expired(future);
        assert!(q.toasts.is_empty(), "info toast should expire");
    }

    #[test]
    fn warning_auto_dismisses_after_warn_ttl() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Warning, "soft".into(), None);
        // INFO_TTL alone is NOT enough to expire a Warning.
        let half = Instant::now() + INFO_TTL + Duration::from_millis(10);
        q.prune_expired(half);
        assert_eq!(q.toasts.len(), 1);
        let later = Instant::now() + WARN_TTL + Duration::from_millis(10);
        q.prune_expired(later);
        assert!(q.toasts.is_empty());
    }

    #[test]
    fn dismiss_removes_by_index() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Info, "a".into(), None);
        q.spawn(ToastKind::Info, "b".into(), None);
        q.dismiss(0);
        assert_eq!(q.toasts.len(), 1);
        assert_eq!(q.toasts[0].text, "b");
        // Out-of-bounds dismiss is a no-op.
        q.dismiss(99);
        assert_eq!(q.toasts.len(), 1);
    }

    #[test]
    fn counts_partitions_by_kind() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Info, "i".into(), None);
        q.spawn(ToastKind::Warning, "w1".into(), None);
        q.spawn(ToastKind::Warning, "w2".into(), None);
        q.spawn(ToastKind::Error, "e".into(), None);
        let (info, warn, err) = q.counts();
        assert_eq!((info, warn, err), (1, 2, 1));
    }

    #[test]
    fn fade_alpha_full_outside_fadeout_window() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Info, "x".into(), None);
        let now = Instant::now();
        assert!((q.toasts[0].fade_alpha(now) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn fade_alpha_ramps_to_zero_in_fadeout_window() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Info, "x".into(), None);
        let dismiss_at = q.toasts[0].dismiss_at.unwrap();
        // 250 ms before dismiss → ~50 % alpha.
        let mid = dismiss_at - FADE_OUT / 2;
        let a = q.toasts[0].fade_alpha(mid);
        assert!(a > 0.3 && a < 0.7, "expected ~0.5, got {a}");
        // At dismiss_at → 0.
        let zero = q.toasts[0].fade_alpha(dismiss_at);
        assert!(zero.abs() < 1e-3);
    }

    #[test]
    fn coalesced_toast_refreshes_dismiss_at() {
        let mut q = ToastQueue::default();
        q.spawn(ToastKind::Info, "x".into(), None);
        let first_dismiss = q.toasts[0].dismiss_at.unwrap();
        std::thread::sleep(Duration::from_millis(5));
        q.spawn(ToastKind::Info, "x".into(), None);
        let second_dismiss = q.toasts[0].dismiss_at.unwrap();
        assert!(
            second_dismiss > first_dismiss,
            "coalesce should bump the dismiss_at forward"
        );
        assert_eq!(q.toasts[0].count, 2);
    }

    #[test]
    fn action_label_round_trip() {
        // Each action variant exposes a non-empty label so the
        // button never renders blank.
        for a in [
            ToastAction::OpenLintPanel,
            ToastAction::OpenBuildLog,
            ToastAction::OpenHelpArticle(HelpArticleId::BuildPipeline),
            ToastAction::DismissMigrationToast,
        ] {
            assert!(!a.label().is_empty());
        }
    }
}
