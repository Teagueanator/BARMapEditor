//! Core data model for the BAR map editor.
//!
//! Crate layout follows the architecture in `docs/ARCHITECTURE.md`:
//! - `project`  — Project root: paths, settings, dirty tracking.
//! - `map_size` — Spring Map Units (SMU) ↔ pixel/elmo conversions.
//! - `heightmap` — Tiled, copy-on-write 16-bit heightmap.

pub mod brushes;
pub mod heightmap;
pub mod map_size;
pub mod procgen;
pub mod project;
pub mod start_pos;
pub mod symmetry;
pub mod undo;

pub use brushes::{Brush, BrushRegistry, BrushStamp, DirtyRect};
pub use heightmap::{DimMismatch, Heightmap};
pub use map_size::MapSize;
pub use procgen::{
    BIOMES, BiomePreset, Domain, PRESETS, ProcGenError, ProcGenPreset, generate as procgen_generate,
};
pub use project::{
    PROJECT_EXTENSION, Project, ProjectLoadError, ProjectSaveError, StartPosition, sanitize_name,
};
pub use symmetry::SymmetryAxis;
pub use undo::{History, StampSnapshot, UndoEntry};
