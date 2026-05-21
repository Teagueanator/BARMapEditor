//! UI helpers that live next to `App` but don't belong inline in
//! `main.rs`. ADR-030 (B1) kept everything inline; ADR-031 (B2)
//! introduces this module dir as the home for canvas overlays —
//! once `central()` would otherwise grow past ~500 lines. B3
//! grows it with `gizmo`, `cheat_sheet`, and `intro`.

pub mod build_overlay;
pub mod cheat_sheet;
pub mod help_text;
pub mod icons;
pub mod inspector_mapinfo;
pub mod intro;
pub mod layers_panel;
pub mod lint_panel;
pub mod markers;
pub mod minimap;
pub mod next_steps;
pub mod overlay;
pub mod paint_view;
pub mod theme;
pub mod viewport_chrome;
pub mod widgets;
