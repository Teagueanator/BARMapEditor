//! Core data model for the BAR map editor.
//!
//! Crate layout follows the architecture in `docs/ARCHITECTURE.md`:
//! - `project`  — Project root: paths, settings, dirty tracking.
//! - `map_size` — Spring Map Units (SMU) ↔ pixel/elmo conversions.
//! - `heightmap` — Tiled, copy-on-write 16-bit heightmap.

pub mod heightmap;
pub mod map_size;
pub mod project;

pub use heightmap::{DimMismatch, Heightmap};
pub use map_size::MapSize;
pub use project::Project;
