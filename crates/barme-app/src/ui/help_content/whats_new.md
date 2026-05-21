# What's new

## Sprint 22 (2026-05-20) — Onboarding loop closure

This release closes the U2 discoverability gap from the
2026-05-20 audit. Highlights:

- **Help center** — the persistent Help icon in the top bar opens
  this Window any time. Articles per tool, per pitfall, plus
  meta articles like Getting Started and Build Pipeline.
- **Guided tour** — a 7-step walkthrough auto-triggers on the
  first new project, and is re-runnable from the Help menu.
- **Per-tool intros** — entering a tool for the first time pops a
  short non-modal overlay with the essentials.
- **Ctrl+K command palette** — fuzzy-search every tool, preset,
  menu item, and keyboard shortcut.
- **Ctrl+Shift+H what's-this mode** — a cursor-pinned popover
  mode that turns the Sprint 19 hover tooltips into persistent
  popovers with "Read more" links into the help center.

## Sprint 21 (2026-05-20) — Lint pass

Project lint now evaluates 20+ mapinfo rules on every frame and
surfaces them in a dedicated panel reached from the top-bar
validation chip. Errors block the build; warnings are advisory.
Most rules ship with an undoable one-click fix.

## Sprint 20 (2026-05-20) — Async build + log

The build pipeline moved off the UI thread. The progress overlay
shows live stage + percentage; the log panel streams PyMapConv
output as it arrives. Builds are cancellable.

## Sprint 19 (2026-05-20) — UI tooltip + help-text pass

Every interactive widget grew a hover-tooltip pulled from a
centralised `HelpId` catalogue (100+ entries). Suffixes like
`[Shortcut: …]` and `[PITFALL §N]` keep the chord and pitfall
references aligned with this help center.
