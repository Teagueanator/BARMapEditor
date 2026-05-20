//! Core data model for the BAR map editor.
//!
//! Crate layout follows the architecture in `docs/ARCHITECTURE.md`:
//! - `project`  — Project root: paths, settings, dirty tracking.
//! - `map_size` — Spring Map Units (SMU) ↔ pixel/elmo conversions.
//! - `heightmap` — Tiled, copy-on-write 16-bit heightmap.

pub mod brushes;
pub mod heightmap;
pub mod layers;
pub mod map_size;
pub mod mapinfo_schema;
pub mod procgen;
pub mod project;
pub mod splat;
pub mod start_pos;
pub mod symmetry;
pub mod undo;
pub mod water_presets;

pub use brushes::{Brush, BrushRegistry, BrushStamp, DirtyRect};
pub use heightmap::{DimMismatch, Heightmap};
pub use layers::{
    BlendMode, ClosureSlotResolver, LayerColor, LayerMask, LayerSource, LayerStack, LayerTransform,
    MaskBrush, MaskBrushRegistry, MaskFill, MaskHide, MaskReveal, MaskSmooth, MaskStamp,
    SlotResolver, TILE_DIM, TILE_PIXELS, TextureLayer, TileCoord, alloc_layer_id,
};
pub use map_size::MapSize;
pub use mapinfo_schema::{
    AtmosphereBlock, GrassBlock, LightingBlock, MapInfo, ResourcesBlock, Rgb, SmfBlock, SoundBlock,
    SplatsBlock, SunDir, TeamBlock, TeamStartPos, TerrainMoveSpeeds, TerrainTypeBlock, WaterBlock,
};
pub use procgen::{
    BIOMES, BiomePreset, Domain, PRESETS, ProcGenError, ProcGenPreset, generate as procgen_generate,
};
pub use project::{
    ALLY_GROUP_PALETTE, AllyGroup, FeatureInstance, GeoVent, MetalSpot, PROJECT_EXTENSION, Project,
    ProjectLoadError, ProjectSaveError, StartPosition, default_extractor_radius, sanitize_name,
};
pub use splat::{
    Erase as SplatErase, PaintChannel, SPLAT_DIM, Smooth as SplatSmooth, SplatBrush,
    SplatBrushRegistry, SplatChannel, SplatConfig, SplatDistribution, SplatStamp,
};
pub use symmetry::SymmetryAxis;
pub use undo::{
    HeightmapEntry, History, HistoryEntry, LayerPropertyValue, ProjectDiff, WaterField, WaterValue,
    WizardSnapshot,
};
pub use water_presets::{
    BAR_DEFAULT_SURFACE_ALPHA, BAR_DEFAULT_SURFACE_COLOR, WaterMode, apply_lava_atmosphere_patch,
    merge_overrides, preset_water_block, water_override_count,
};
