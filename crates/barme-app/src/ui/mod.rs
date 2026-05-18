//! UI helpers that live next to `App` but don't belong inline in
//! `main.rs`. ADR-030 (B1) kept everything inline; ADR-031 (B2)
//! introduces this module dir as the home for canvas overlays — once
//! `central()` would otherwise grow past ~500 lines.

pub mod overlay;
