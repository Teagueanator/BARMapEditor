# Sprint 31 — Toast queue + confirmation modals + dirty-state guards (U4)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 31** — a focused UX-polish sprint that finally
delivers a proper **toast queue** and **confirmation modal** primitive.
The 2026-05-20 UX audit found:

- The single `last_error` line in the status strip is the ONLY
  toast channel. It accumulates and only shows the most recent.
- `pending_migration_toast` (Sprint 17) ships a bespoke
  `egui::Window` for ONE specific case. The pattern hasn't been
  generalised.
- Sprint 20's "save before build" guard uses a stub Window pattern
  that this sprint replaces with the real primitive.
- **No confirmation on destructive actions**. Delete-ally-group
  silently destroys all nested start positions. Delete-layer
  destroys mask data. New-project / Open-project discards unsaved
  changes without prompting.

After this sprint:

- A **`Toast` queue** primitive: Info / Warn / Error toasts with
  auto-dismiss (Info: 3s; Warn: 6s; Error: persistent).
- All 12+ `self.last_error = Some(...)` call sites migrate to the
  toast queue.
- A **`confirm_modal()` primitive** for destructive actions.
- Confirmation modals wired into: delete ally group with N
  positions, delete layer with mask data, new-project/open-project
  with unsaved changes.

**Prerequisites:**
- Sprint 30 (shadows) MUST be ticked. The toast queue uses Sprint
  19's panel primitive — established now.
- Sprint 22 (onboarding + help center) MUST be ticked. The help
  center's articles link to confirmation rationale.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.3 NFR-Reliability
   (destructive operations should be reversible or confirmed).
3. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs`
   — search `self.last_error = Some` for ~12 call sites that need
   migration. Search `delete_*` for the destructive paths.
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/widgets.rs`
   — extend with `toast_render` + `confirm_modal`.
5. Existing patterns:
   - `crates/barme-app/src/main.rs::pending_migration_toast` —
     the bespoke Window pattern. Lift this into the new toast
     primitive.
   - `crates/barme-app/src/ui/build_overlay.rs` (Sprint 20) — uses
     `egui::Area`. Same approach for toasts.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-31-toast-queue
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. `Toast` primitive

**New module:** `crates/barme-app/src/ui/toast.rs`.

```rust
pub struct ToastQueue {
    pub toasts: VecDeque<Toast>,
}

pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub spawned_at: Instant,
    pub dismiss_at: Option<Instant>,  // None = persistent
    pub action: Option<ToastAction>,  // optional "Click to do X"
}

pub enum ToastKind {
    Info,
    Warning,
    Error,
}

pub enum ToastAction {
    OpenLintPanel,
    OpenBuildLog,
    OpenHelpArticle(HelpArticleId),
    // ...
}
```

**Render**: floating `egui::Area` anchored to bottom-right of the
viewport. Stack vertically. Each toast has:
- Icon (info / warn / error).
- Text.
- Dismiss button (×).
- Action button (if `action.is_some()`).
- Auto-fade-out animation in the last 500 ms before dismiss.

**Methods**:
- `App::toast_info(text)` / `App::toast_warn(text)` /
  `App::toast_error(text)` — common-case constructors.
- `App::toast_with_action(kind, text, action)`.
- Auto-dismiss timing: Info = 3s, Warn = 6s, Error = persistent
  (until user dismisses).

### 2. Migrate `last_error` → toast queue

Audit every `self.last_error = Some(...)` site. There are ~12:
- `crates/barme-app/src/main.rs:2711` — texture import (save
  project first).
- `crates/barme-app/src/main.rs:2733` — import failed.
- `crates/barme-app/src/main.rs:2766` — texture save failed.
- (continue grepping for the rest)

Each becomes `self.toast_error(message)` (or `toast_warn` /
`toast_info`).

Remove `App::last_error: Option<String>` field.

The status strip's "last error" position (`main.rs:6049-6052`) is
replaced by a count of active toasts (clickable → opens a
"Recent toasts" log).

### 3. `confirm_modal()` primitive

**New module:** `crates/barme-app/src/ui/confirm.rs`.

```rust
pub struct ConfirmDialog {
    pub title: &'static str,
    pub message: String,
    pub confirm_label: &'static str,
    pub cancel_label: &'static str,
    pub destructive: bool,  // tints confirm button red
}

pub enum ConfirmResult { Confirmed, Cancelled }

pub fn confirm_modal(
    ctx: &egui::Context,
    state: &mut Option<ConfirmDialog>,
) -> Option<ConfirmResult>;
```

Pattern: caller sets `App::pending_confirm = Some(ConfirmDialog
{...})`; the modal renders next frame; the user clicks
Confirm/Cancel; `confirm_modal` returns `Some(result)` once on
the resolving frame, then clears the dialog.

**Visual**: centred modal with darkened backdrop. Esc = cancel.
Enter = confirm. Confirm button styled red if `destructive`.

### 4. Wire confirmation modals

Audit destructive actions:
- **Delete ally group**: if `group.sources.len() > 0`, confirm
  "Delete ally group '{}' AND its {} start positions?".
- **Delete layer with mask data**: if layer's mask has any
  non-uniform tiles, confirm "Delete layer '{}' (contains
  painted mask data)?".
- **New project**: if `App::dirty`, confirm "Discard unsaved
  changes to {}?".
- **Open project**: same as new-project guard.
- **Discard recent project**: minor; no confirm needed.
- **Clear undo history** (if exposed): confirm.
- **Build with errors**: handled by Sprint 21 (lint panel block);
  no toast/modal change.

All these go through `confirm_modal`. Sprint 20's "save before
build" guard's stub Window also migrates here.

### 5. Toast surfaces from existing systems

- **Sprint 20 build**: replace `BuildState::Done(Ok(path))` notification
  with `toast_info("Built {} in {:?}", path.display(), duration)`.
- **Sprint 20 build**: `BuildState::Failed` shows
  `toast_error_with_action("Build failed", OpenBuildLog)`.
- **Sprint 21 lint**: when new lint issues land, optionally
  toast_warn the count (rate-limited; once per 5s).
- **Sprint 23 GC**: `toast_info("Garbage-collected {} orphan
  textures", count)`.
- **Sprint 19 validation chip**: on click, also fire
  `toast_info_with_action("Showing lint issues", OpenLintPanel)`
  for confirmation.

### 6. Tests + smoke + rollup

- **Toast queue tests**: spawn 5 toasts, advance time, verify
  auto-dismiss order; persistent error stays.
- **Confirm dialog tests**: render with Enter/Esc, verify
  result returns.
- **Migration smoke**: `grep -r 'last_error' crates/` should
  return zero hits in production code (test fixtures may keep).

- **Rollup commit**: STATUS UPDATEs in SRS / ROADMAP (U4 done).
  closing devlog log. "Sprint 32 = F12 Launch-in-BAR + autosave"
  handoff note.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `trace!` on toast spawn/dismiss;
`trace!` on confirm-dialog open/resolve.

## Step 5 — Out of scope

- **Undo of toast actions** — toasts are notifications, not undo
  records. The underlying ProjectDiff handles undo.
- **Persistent toast log across editor sessions** — toast queue
  is in-memory only.
- **Notification sounds** — visual-only. Stage-2 polish.
- **Programmatic toast dismissal from background tasks** — only
  user-driven dismissal.
- **Touch gestures (swipe to dismiss)** — out of scope.

## Step 6 — Critical pitfalls (read twice)

1. **Sprint 17 `pending_migration_toast` is the existing
   pattern** — lift carefully, don't break the migration UX.
   Convert to a regular toast with `kind = Info, dismiss_at =
   None` and a "Don't show again" action.

2. **Don't toast-spam**. Rate-limit duplicate toasts: if
   `toast_warn("X")` is called twice within 5 seconds with the
   same text, replace the first toast's text with "X (×2)"
   instead of stacking.

3. **Toast queue cap**: hard-cap at 10 active toasts. Beyond
   that, drop oldest non-error toasts. Errors are never auto-
   dropped.

4. **Confirm modal Enter/Esc**: must not eat all keyboard
   events. Only Enter (confirm) and Esc (cancel) — other keys
   pass through. The modal Window uses `egui::Window::movable(false)`
   + an opaque backdrop layer to block click-through but not
   keyboard.

5. **Destructive button colour**: use `t.danger` red on the
   confirm button when `destructive = true`. Cancel button
   stays neutral (`t.muted`).

6. **Build-completion toast action**: clicking the toast should
   NOT auto-open the build log if the user clicked elsewhere.
   The action is the explicit `[View log]` button on the toast.

7. **Sprint 20 "save before build" stub Window**: migrate
   carefully. The stub used a synchronous wait-for-input pattern;
   the new pattern is fire-and-forget (set
   `pending_confirm = Some(...)`, then the next frame resolves).
   Restructure the call site so the build kicks off only after
   the confirm result lands.

8. **Don't break Ctrl+Z**: confirm modals do NOT add undo entries.
   The action they confirm does (via ProjectDiff). The modal
   itself is transient state.

9. **Toast pinning**: persistent error toasts (`dismiss_at =
   None`) MUST have a dismiss button. The user can dismiss any
   toast manually.

10. **Migration `Project.migration_toast_dismissed`**: Sprint 17
    persisted this per-project. Sprint 31 changes the toast
    delivery mechanism but should NOT change the persistence
    contract — the user's "Don't show again" choice should still
    survive across sessions.

11. **`egui::Window` modal-ness**: egui doesn't have true modals.
    Use `Area::order(Order::Foreground)` + a fullscreen backdrop
    layer at `Order::Foreground - 1` that catches clicks. The
    confirm dialog is a child Window of the backdrop area.

## Step 7 — Exit criteria

- 4+ commits on `main`: toast primitive, migration of last_error
  call sites, confirm modal primitive, wire confirmations,
  rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (U4 done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - Spawn 5 toasts of different kinds → render correctly with
    auto-dismiss.
  - Delete an ally group with positions → confirmation modal
    fires; cancel returns no-op; confirm deletes.
  - New project with unsaved changes → confirmation modal.
  - Build success → info toast; build failure → error toast
    with View log action.
- Final devlog: summary + "Sprint 32 = F12 Launch + autosave"
  handoff.

Start by writing the `Toast` + `ToastQueue` primitive. Then
migrate the `last_error` call sites mechanically (one commit per
~4 sites). Then ship the confirm modal + wire it.
