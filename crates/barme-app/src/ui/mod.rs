//! UI helpers that live next to `App` but don't belong inline in
//! `main.rs`. ADR-030 (B1) kept everything inline; ADR-031 (B2)
//! introduces this module dir as the home for canvas overlays —
//! once `central()` would otherwise grow past ~500 lines. B3
//! grows it with `gizmo`, `cheat_sheet`, and `intro`.

pub mod cheat_sheet;
pub mod gizmo;
pub mod intro;
pub mod overlay;
