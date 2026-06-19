//! Unified undo / redo over heightmap strokes AND project-level edits.
//!
//! ADR-033 introduced per-stroke copy-on-first-write for heightmap edits.
//! B5 keeps that machinery verbatim and grows the data model around it:
//! the history stack now holds an enum of either a heightmap stroke or a
//! project-level diff (start-position place/move/delete, wizard apply).
//! One stack, one Ctrl-Z hits whichever entry sits on top.
//!
//! ## Eviction policy — largest-first, not strictly oldest
//!
//! Heightmap entries are bytes-heavy (a 16-SMU stroke at radius 1024 can
//! commit ~2 MB). Project diffs are kilobytes at most (Place/Delete are
//! tens of bytes; ApplyWizard with a few hundred start positions tops out
//! in the low kilobytes). Under the previous "evict the oldest" rule a
//! single long stroke would happily evict the last 20 F8 placements just
//! to make room. Sharing a 100 MB cap is still fine — the budget
//! comfortably holds ~50 long strokes — but eviction has to prefer the
//! *largest* committed entry, otherwise the cap lopsidedly punishes the
//! lighter channel.
//!
//! ## Brush-stroke capture path (unchanged from ADR-033)
//!
//! The in-flight stroke state — heightmap-sized scratch buffer + packed
//! bitset — lives inside [`History`] and is committed via
//! [`History::end_stroke`] into a [`HeightmapEntry`]. See the original
//! ADR-033 prose for the rationale; B5 doesn't touch any of it.
//!
//! ## Project-diff capture path (B5)
//!
//! Callers push project diffs directly with
//! [`History::push_project_diff`]. The data shape is symmetric: each
//! variant carries enough state to apply itself in either direction, and
//! `apply` on undo means "revert this", `apply` on redo means "re-do
//! this." The app dispatches the variant via [`History::pop_undo`] /
//! [`History::pop_redo`] + [`History::push_to_redo`] /
//! [`History::push_to_undo`] — the project mutation lives in the app
//! because the field set crosses the barme-core / barme-app boundary.

use std::collections::VecDeque;

use tracing::{trace, warn};

use std::collections::HashMap;

use crate::MapSize;
use crate::brushes::DirtyRect;
use crate::heightmap::Heightmap;
use crate::layers::{
    BlendMode, LayerColor, LayerMask, LayerSource, LayerTransform, TextureLayer, Tile, TileCoord,
};
use crate::mapinfo_schema::MapInfoPatch;
use crate::procgen::Domain;
use crate::project::{AllyGroup, FeatureInstance, GeoVent, MetalSpot, StartPosition};
use crate::splat::SplatChannel;
use crate::symmetry::SymmetryAxis;
use crate::water_presets::WaterMode;

/// One committed undo entry. Either a heightmap stroke (the ADR-033
/// shape), a layer-mask stroke (the Sprint 17 / ADR-041 adaptation
/// of ADR-033 onto tiled-COW masks), or a project-level diff.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryEntry {
    Heightmap(HeightmapEntry),
    /// D10 / Sprint 17 (ADR-041) — one brush stroke against a layer
    /// mask. Stored as a sparse map of (tile coord, before, after)
    /// snapshots. Undo restores `before`; redo replays `after`.
    Mask(MaskEntry),
    Project(ProjectDiff),
}

impl HistoryEntry {
    /// Approximate bytes this entry occupies, used by the cap eviction.
    /// Counts heap allocations explicitly; small inline state (enum
    /// tags, fixed-size fields) is approximated by `size_of_val`.
    pub fn bytes(&self) -> usize {
        match self {
            HistoryEntry::Heightmap(h) => h.bytes(),
            HistoryEntry::Mask(m) => m.bytes(),
            HistoryEntry::Project(p) => p.bytes(),
        }
    }
}

/// D10 / Sprint 17 (ADR-041) — one layer-mask stroke. The Sprint 16
/// tiled-COW mask makes a flat byte snapshot wasteful (you'd pay
/// 64 MB even for a 16-px stamp); the entry instead stores per-tile
/// before/after snapshots scoped to the tiles the brush touched.
///
/// `tiles` only contains entries where `before != after` — a no-op
/// stroke gets filtered out at commit time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskEntry {
    /// Layer the stroke targeted. Stored as the layer id (stable
    /// across reorders) rather than a vec index.
    pub layer_id: String,
    /// Per-tile before/after pairs. Order is committed-into-vec order
    /// (insertion order of the open stroke).
    pub tiles: Vec<(TileCoord, Tile, Tile)>,
}

impl MaskEntry {
    pub fn bytes(&self) -> usize {
        let per_tile: usize = self
            .tiles
            .iter()
            .map(|(_, b, a)| b.resident_bytes() + a.resident_bytes())
            .sum();
        self.layer_id.capacity() + per_tile
    }
}

/// One heightmap stroke. The bbox is the union of every pixel the stroke
/// touched (computed at `end_stroke` from the per-stroke bitset);
/// `before` holds the bbox-shaped pre-stroke pixels in row-major order.
/// After [`Heightmap::swap_rect`] the same buffer holds the *current*
/// (post-undo) pixels and can be replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightmapEntry {
    pub rect: DirtyRect,
    pub before: Vec<u16>,
}

impl HeightmapEntry {
    pub fn bytes(&self) -> usize {
        self.before.len() * std::mem::size_of::<u16>()
    }
}

/// One project-level diff. Each variant is reversible: on undo, the app
/// applies the *inverse* and pushes that inverse onto the redo stack;
/// on redo, the mirror. Inversion lives in the app because Wizard
/// snapshots cross the barme-core / barme-app boundary (project name +
/// height scale live on `App`, not `Project`).
///
/// **ADR-032 (B6):** position identifiers are `(ally_group_id, pos)`
/// not `team_id`. The flat `team_id` was a property of the
/// pre-Phase-3 model. Now a position is uniquely identified by the
/// group it lives in and its coordinates (coords are unique within a
/// group by construction — the F8 logic refuses duplicate placements).
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectDiff {
    /// A start position was added to `ally_group_id` at `pos`. Undo
    /// removes it; redo re-adds.
    PlaceStartPosition {
        ally_group_id: u8,
        pos: StartPosition,
    },
    /// A start position was deleted from `ally_group_id`. Undo
    /// re-adds; redo removes.
    DeleteStartPosition {
        ally_group_id: u8,
        pos: StartPosition,
    },
    /// A start position in `ally_group_id` was moved from `from` to
    /// `to`. Undo restores `from`; redo restores `to`. The drag
    /// finalizer collapses an entire drag gesture into one entry.
    MoveStartPosition {
        ally_group_id: u8,
        from: StartPosition,
        to: StartPosition,
    },
    /// The F1 wizard replaced project-level state wholesale. The boxed
    /// snapshot holds the *pre-wizard* state; on undo the app swaps it
    /// with the *current* (post-wizard) state and pushes the captured
    /// post-wizard snapshot onto the redo stack.
    ApplyWizard(Box<WizardSnapshot>),
    /// C4 (Sprint 11): a metal-spot source was added. Identity is the
    /// full `MetalSpot` record (coords + metal value). Undo removes
    /// the matching source; redo re-adds. Each LMB-click in
    /// `Tool::MetalSpots` produces one diff per source (symmetry
    /// mirrors each push their own diff so undo peels mirrors one at
    /// a time — matches F8 / B5 semantics).
    PlaceMetalSpot { spot: MetalSpot },
    /// A metal-spot source was deleted. Holds the full pre-delete
    /// record so undo can re-add it verbatim.
    DeleteMetalSpot { spot: MetalSpot },
    /// A metal-spot source's coordinates and/or metal value changed
    /// (drag-move on canvas, or DragValue edit in the inspector).
    /// `from` is the pre-edit record; `to` is post-edit. Identity is
    /// `from` for undo / `to` for redo. The drag finalizer collapses
    /// an entire drag gesture into a single entry.
    MoveMetalSpot { from: MetalSpot, to: MetalSpot },
    /// The `extractor_radius` global was edited. Symmetric inverse:
    /// undo restores `from`, redo restores `to`. Inspector edits
    /// collapse into one entry per commit (slider drag-release).
    SetExtractorRadius { from: f32, to: f32 },
    /// C5 (Sprint 11): a geo-vent source was added. See
    /// [`Self::PlaceMetalSpot`] for the symmetry / per-mirror diff
    /// semantics — identical.
    PlaceGeoVent { vent: GeoVent },
    /// A geo-vent source was deleted.
    DeleteGeoVent { vent: GeoVent },
    /// A geo-vent source's coordinates changed (drag-move / inspector
    /// edit).
    MoveGeoVent { from: GeoVent, to: GeoVent },
    /// C6 (Sprint 12): a user-placed feature source was added. Identity
    /// is the full `FeatureInstance` (name + coords + rotation). Per
    /// the F7 dispatch the diff pushes one entry per symmetry mirror so
    /// undo peels mirrors one at a time, matching the metal/geo
    /// convention.
    PlaceFeature { feature: FeatureInstance },
    /// A feature source was deleted.
    DeleteFeature { feature: FeatureInstance },
    /// A feature source's coords and/or rotation changed (drag-move /
    /// drag-rotate on canvas, or inspector edit). The drag finalizer
    /// collapses the gesture into a single entry.
    MoveFeature {
        from: FeatureInstance,
        to: FeatureInstance,
    },
    /// C9 (Sprint 14 / ADR-042): the active water preset changed.
    /// Inspector dispatches one diff per preset chip click; undo
    /// restores `from`, redo restores `to`. `Project.water_overrides`
    /// is deliberately preserved across the change (Photoshop-style
    /// behaviour — user tweaks bleed through preset swaps).
    SetWaterMode { from: WaterMode, to: WaterMode },
    /// C9 (Sprint 14 / ADR-042): an individual water-block field
    /// (slider, color picker, toggle) was edited. `field` identifies
    /// the target; `from` / `to` are the symmetric values for undo +
    /// redo. The drag finalizer collapses an entire slider drag into
    /// one entry.
    ///
    /// Two `WaterField` variants — `VoidWater` and `TidalStrength` —
    /// address `Project.void_water` / `Project.tidal_strength`
    /// rather than fields inside `Project.water_overrides`; the
    /// inspector co-locates them with the water-block controls but
    /// the schema field lives at MapInfo top level.
    EditWaterField {
        field: WaterField,
        from: WaterValue,
        to: WaterValue,
    },
    /// C9 (Sprint 14 / ADR-042): the lava-atmosphere offer was
    /// flipped. Coarser than per-field atmosphere editing — the
    /// `Project.lava_atmosphere: bool` gates a hardcoded
    /// fog/sun/cloud patch the emission path applies on top of
    /// `bar_default()`. Sprint 18's F9 form will eventually offer
    /// per-field atmosphere overrides.
    SetLavaAtmosphere { from: bool, to: bool },
    /// D8 / Sprint 15 (ADR-038): a [`TextureLayer`] was added at
    /// `index` in `Project.layers`. Undo removes it; redo re-adds the
    /// stored layer verbatim. The full layer snapshot lives in the
    /// diff so the mask + transform survive the undo round-trip.
    /// Mask-pixel edits are NOT undoable in Sprint 15 (no brushes
    /// touch the mask yet) — Sprint 16 / D9 lands a separate per-
    /// stroke COW path adapted from ADR-033.
    AddLayer {
        index: usize,
        layer: Box<TextureLayer>,
    },
    /// D8 / Sprint 15: a [`TextureLayer`] was removed from `index`.
    /// Undo re-inserts; redo removes again. Captures the full layer
    /// including mask bytes so the undo restores the pre-delete state
    /// byte-for-byte.
    RemoveLayer {
        index: usize,
        layer: Box<TextureLayer>,
    },
    /// D8 / Sprint 15: layer at `from` moved to `to`. Symmetric on
    /// undo (swap `from` / `to`).
    ReorderLayer { from: usize, to: usize },
    /// D8 / Sprint 15: one non-mask property on the layer identified
    /// by `layer_id` changed. The dispatcher (in `barme-app`) walks
    /// `Project.layers.layers` for the matching `id` and applies
    /// `from` on undo / `to` on redo. Mask edits are excluded —
    /// they have their own (Sprint 16) per-stroke path.
    SetLayerProperty {
        layer_id: String,
        from: LayerPropertyValue,
        to: LayerPropertyValue,
    },
    /// C7 / Sprint 18 (F9): one leaf field on the typed
    /// [`crate::MapInfo`] schema (or its App-side shadow on `Project`,
    /// like `Project.minimap_override` /
    /// `Project.lava_atmosphere`) was edited via the F9 form. `from`
    /// holds the pre-edit value; `to` the post-edit. The discriminant
    /// is identical between `from` and `to` (the dispatcher debug-asserts
    /// this) so the form's variant pick determines the target field
    /// uniquely.
    ///
    /// Why a separate variant from [`Self::EditWaterField`]: the
    /// water-block editor (Sprint 14 / C9) was scoped to the dedicated
    /// `Tool::Water` Inspector with a sparse-Option overlay and
    /// per-preset reset semantics that don't fit MapInfo's flat-leaf
    /// shape. F9's "Water" tab is a power-user backstop that drives
    /// the SAME `Project.water_overrides` — its DragValue commits
    /// still emit `EditWaterField`, not `EditMapInfo`. Keeps the two
    /// undo streams aligned: any water-block edit, no matter where the
    /// user made it, undoes the same way.
    EditMapInfo {
        from: MapInfoPatch,
        to: MapInfoPatch,
    },
}

/// D8 / Sprint 15 (ADR-038): typed-union value carried by
/// [`ProjectDiff::SetLayerProperty`]. Each variant identifies which
/// scalar / sub-struct on [`TextureLayer`] is being edited. Mask
/// bytes are intentionally absent — they go through a separate
/// Sprint 16 path.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerPropertyValue {
    Name(String),
    Transform(LayerTransform),
    Color(LayerColor),
    Blend(BlendMode),
    Visible(bool),
    Locked(bool),
    Opacity(f32),
    DntsChannel(Option<SplatChannel>),
    Source(LayerSource),
    /// D10 / Sprint 17 (ADR-041): per-layer DNTS `texScale`. Only
    /// meaningful when the layer carries a `DntsChannel(Some(_))`
    /// binding; the diff still rides through undo when the binding is
    /// `None` so a user toggling the binding and tweaking the slider in
    /// any order produces a consistent history.
    DntsTexScale(f32),
    /// D10 / Sprint 17 (ADR-041): per-layer DNTS `texMult`. See
    /// [`Self::DntsTexScale`].
    DntsTexMult(f32),
}

/// C9 / Sprint 14: which water-related field an [`ProjectDiff::EditWaterField`]
/// targets. Most variants address fields inside
/// `Project.water_overrides`; `VoidWater` and `TidalStrength` address
/// MapInfo top-level fields (`Project.void_water` /
/// `Project.tidal_strength`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaterField {
    Damage,
    SurfaceColor,
    SurfaceAlpha,
    PlaneColor,
    Absorb,
    BaseColor,
    MinColor,
    AmbientFactor,
    DiffuseFactor,
    SpecularFactor,
    SpecularColor,
    SpecularPower,
    FresnelMin,
    FresnelMax,
    FresnelPower,
    ReflectionDistortion,
    BlurBase,
    BlurExponent,
    PerlinStartFreq,
    PerlinLacunarity,
    PerlinAmplitude,
    WaveFoamIntensity,
    NumTiles,
    ShoreWaves,
    ForceRendering,
    RepeatX,
    RepeatY,
    /// Lives on `Project.void_water` (MapInfo top-level `voidWater`).
    /// The dispatcher must read/write `Project.void_water: bool`, NOT
    /// `Project.water_overrides`.
    VoidWater,
    /// Lives on `Project.tidal_strength` (MapInfo top-level
    /// `tidalStrength`). Dispatcher reads/writes
    /// `Project.tidal_strength: Option<f32>`.
    TidalStrength,
}

/// C9 / Sprint 14: tagged-union value carried by
/// [`ProjectDiff::EditWaterField`]. Wrapping `Option<T>` lets the
/// diff naturally express "clear this override" (set the field back
/// to `None` — i.e. fall through to the preset baseline).
///
/// `VoidWater` is the one non-optional field; its diffs always use
/// `Bool(Some(_))` and the dispatcher unwraps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaterValue {
    Float(Option<f32>),
    Rgb(Option<[f32; 3]>),
    Bool(Option<bool>),
    UInt(Option<u32>),
}

impl ProjectDiff {
    pub fn bytes(&self) -> usize {
        let inline = std::mem::size_of::<Self>();
        match self {
            ProjectDiff::PlaceStartPosition { .. }
            | ProjectDiff::DeleteStartPosition { .. }
            | ProjectDiff::MoveStartPosition { .. }
            | ProjectDiff::PlaceMetalSpot { .. }
            | ProjectDiff::DeleteMetalSpot { .. }
            | ProjectDiff::MoveMetalSpot { .. }
            | ProjectDiff::SetExtractorRadius { .. }
            | ProjectDiff::PlaceGeoVent { .. }
            | ProjectDiff::DeleteGeoVent { .. }
            | ProjectDiff::MoveGeoVent { .. }
            | ProjectDiff::SetWaterMode { .. }
            | ProjectDiff::EditWaterField { .. }
            | ProjectDiff::SetLavaAtmosphere { .. }
            | ProjectDiff::ReorderLayer { .. } => inline,
            ProjectDiff::ApplyWizard(snap) => inline + snap.bytes(),
            // Feature variants carry a String `name`; the heap cost of
            // its capacity is in addition to the enum inline size.
            ProjectDiff::PlaceFeature { feature } | ProjectDiff::DeleteFeature { feature } => {
                inline + feature.name.capacity()
            }
            ProjectDiff::MoveFeature { from, to } => {
                inline + from.name.capacity() + to.name.capacity()
            }
            // D8 / Sprint 15 (ADR-038): the heap cost of a layer diff
            // is dominated by the mask bytes (`width * height`) plus
            // the source path / id strings. The cap eviction relies on
            // honest accounting here — a 16-SMU mask is 64 MB and
            // would otherwise sandbag the 100 MB undo cap.
            ProjectDiff::AddLayer { layer, .. } | ProjectDiff::RemoveLayer { layer, .. } => {
                inline + layer_bytes(layer)
            }
            ProjectDiff::SetLayerProperty {
                layer_id, from, to, ..
            } => {
                inline + layer_id.capacity() + layer_property_bytes(from) + layer_property_bytes(to)
            }
            // C7 / Sprint 18 (F9): every MapInfoPatch variant the F9
            // form emits carries small leaves (scalars, RGB triples,
            // string overrides) — total weight is dominated by any
            // heap-allocated `String` / `PathBuf` / `Vec<TerrainTypeBlock>`
            // inside the variant. We approximate via the inline enum
            // size + the heap capacity of the worst-case-known
            // variants. The undo cap can absorb hundreds of
            // f32-leaf edits without re-budgeting.
            ProjectDiff::EditMapInfo { from, to } => {
                inline + mapinfo_patch_bytes(from) + mapinfo_patch_bytes(to)
            }
        }
    }
}

/// Heap-byte estimate for one [`MapInfoPatch`]. Scalars cost zero
/// (already inline in the enum); strings + path buffers + the
/// terrain-types vec cost their heap footprint.
fn mapinfo_patch_bytes(p: &MapInfoPatch) -> usize {
    match p {
        MapInfoPatch::Name(s) | MapInfoPatch::Version(s) => s.capacity(),
        MapInfoPatch::Shortname(s)
        | MapInfoPatch::Description(s)
        | MapInfoPatch::Author(s)
        | MapInfoPatch::SmfMinimapTex(s)
        | MapInfoPatch::AtmosphereSkyBox(s)
        | MapInfoPatch::ResourcesDetailTex(s)
        | MapInfoPatch::ResourcesSpecularTex(s)
        | MapInfoPatch::ResourcesDetailNormalTex(s)
        | MapInfoPatch::ResourcesLightEmissionTex(s)
        | MapInfoPatch::ResourcesSkyReflectModTex(s)
        | MapInfoPatch::ResourcesParallaxHeightTex(s)
        | MapInfoPatch::ResourcesGrassBladeTex(s) => s.as_ref().map(String::capacity).unwrap_or(0),
        MapInfoPatch::TerrainTypes(v) => {
            // Per-row name + per-row TerrainMoveSpeeds inline. Names
            // are ≤ ~16 chars in practice; bound at 32 B / row plus
            // the vec capacity.
            v.iter()
                .map(|t| t.name.as_ref().map(String::capacity).unwrap_or(0))
                .sum::<usize>()
                + v.capacity() * std::mem::size_of::<crate::TerrainTypeBlock>()
        }
        MapInfoPatch::CustomField { key, .. } => key.capacity(),
        MapInfoPatch::MinimapOverride(p) => p.as_ref().map(|p| p.as_os_str().len()).unwrap_or(0),
        _ => 0,
    }
}

fn layer_bytes(layer: &TextureLayer) -> usize {
    // Sprint 16: mask cost = LIVE tile cost (Uniform tiles ~16 B
    // each, Pixels tiles 64 KB each). A freshly-added empty layer
    // costs ~16 KB on a 16-SMU map regardless of resolution;
    // diffing it via `AddLayer` no longer sandbags the cap with
    // 64 MB of zeroed bytes the way Sprint 15 did.
    let mask = layer.mask.resident_bytes();
    let source = match &layer.source {
        LayerSource::Slot { .. } => 0,
        // OsStr is a thin wrapper; capacity sits on the inner buffer
        // — approximate by the path's string length.
        LayerSource::Imported { path } => path.as_os_str().len(),
    };
    mask + layer.id.capacity() + layer.name.capacity() + source
}

fn layer_property_bytes(v: &LayerPropertyValue) -> usize {
    match v {
        LayerPropertyValue::Name(s) => s.capacity(),
        LayerPropertyValue::Source(LayerSource::Imported { path }) => path.as_os_str().len(),
        _ => 0,
    }
}

/// Project-level state replaced by an F1 wizard apply. Spans the few
/// app-level fields the wizard touches; the heightmap is deliberately
/// excluded (the wizard barriers the stroke history via
/// `apply_procgen()` so pre-wizard heightmap state is unrecoverable
/// anyway — restoring metadata + start positions is the achievable win).
#[derive(Debug, Clone, PartialEq)]
pub struct WizardSnapshot {
    pub project_name: String,
    pub map_size: MapSize,
    pub height_scale: f32,
    pub symmetry: SymmetryAxis,
    pub rotational_fold: u8,
    /// **ADR-032:** the wizard snapshot carries the full ally-group
    /// tree (the pre-Phase-3 flat vec is gone). Undo restores the
    /// entire tree wholesale, including colours, names, and box
    /// polygons.
    pub ally_groups: Vec<AllyGroup>,
    pub procgen_expr: String,
    pub procgen_domain: Domain,
}

impl WizardSnapshot {
    /// Bytes the heap-owned fields occupy. Used by the cap eviction.
    pub fn bytes(&self) -> usize {
        let positions_bytes: usize = self
            .ally_groups
            .iter()
            .map(|g| {
                g.start_positions.capacity() * std::mem::size_of::<StartPosition>()
                    + g.name.capacity()
                    + g.box_polygon
                        .as_ref()
                        .map(|p| p.capacity() * std::mem::size_of::<(f32, f32)>())
                        .unwrap_or(0)
            })
            .sum();
        let name_bytes = self.project_name.capacity();
        let expr_bytes = self.procgen_expr.capacity();
        let groups_bytes = self.ally_groups.capacity() * std::mem::size_of::<AllyGroup>();
        positions_bytes + name_bytes + expr_bytes + groups_bytes
    }
}

/// In-flight heightmap stroke state. Owned by [`History`]; not exposed.
struct OpenStroke {
    width: u32,
    height: u32,
    scratch: Vec<u16>,
    snapped: Vec<u64>,
}

impl OpenStroke {
    fn new(width: u32, height: u32) -> Self {
        let pixels = (width as usize) * (height as usize);
        let words = pixels.div_ceil(64);
        Self {
            width,
            height,
            scratch: vec![0u16; pixels],
            snapped: vec![0u64; words],
        }
    }

    fn matches(&self, hm: &Heightmap) -> bool {
        self.width == hm.width() && self.height == hm.height()
    }

    fn snapshot_rect(&mut self, hm: &Heightmap, rect: DirtyRect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        debug_assert!(rect.x + rect.w <= self.width);
        debug_assert!(rect.y + rect.h <= self.height);
        let stride = self.width as usize;
        let src = hm.data();
        for row in 0..rect.h {
            let y = (rect.y + row) as usize;
            for col in 0..rect.w {
                let x = (rect.x + col) as usize;
                let idx = y * stride + x;
                let word = idx / 64;
                let bit = idx % 64;
                let mask = 1u64 << bit;
                if (self.snapped[word] & mask) == 0 {
                    self.scratch[idx] = src[idx];
                    self.snapped[word] |= mask;
                }
            }
        }
    }

    fn snapped_bbox(&self) -> Option<DirtyRect> {
        let stride = self.width as usize;
        let total = stride * (self.height as usize);
        let mut min_x = u32::MAX;
        let mut min_y = u32::MAX;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        let mut any = false;
        for (word_idx, &word) in self.snapped.iter().enumerate() {
            if word == 0 {
                continue;
            }
            let base = word_idx * 64;
            let mut w = word;
            while w != 0 {
                let bit = w.trailing_zeros() as usize;
                let idx = base + bit;
                if idx < total {
                    let y = (idx / stride) as u32;
                    let x = (idx % stride) as u32;
                    if x < min_x {
                        min_x = x;
                    }
                    if x > max_x {
                        max_x = x;
                    }
                    if y < min_y {
                        min_y = y;
                    }
                    if y > max_y {
                        max_y = y;
                    }
                    any = true;
                }
                w &= w - 1;
            }
        }
        if !any {
            return None;
        }
        Some(DirtyRect {
            x: min_x,
            y: min_y,
            w: max_x - min_x + 1,
            h: max_y - min_y + 1,
        })
    }

    fn build_before(&self, hm: &Heightmap, rect: DirtyRect) -> Vec<u16> {
        let stride = self.width as usize;
        let src = hm.data();
        let mut out = Vec::with_capacity((rect.w as usize) * (rect.h as usize));
        for row in 0..rect.h {
            let y = (rect.y + row) as usize;
            for col in 0..rect.w {
                let x = (rect.x + col) as usize;
                let idx = y * stride + x;
                let word = idx / 64;
                let bit = idx % 64;
                let mask = 1u64 << bit;
                if (self.snapped[word] & mask) != 0 {
                    out.push(self.scratch[idx]);
                } else {
                    out.push(src[idx]);
                }
            }
        }
        out
    }
}

/// D10 / Sprint 17 (ADR-041) — in-flight layer-mask stroke. Mirrors
/// the heightmap-side [`OpenStroke`] but at tile granularity: each
/// distinct `TileCoord` the brush touches gets a single pre-stroke
/// snapshot, regardless of how many stamps land in it.
struct OpenMaskStroke {
    layer_id: String,
    /// Pre-stroke tile state captured on the first stamp that
    /// touches each tile. Subsequent stamps re-find the entry and
    /// leave it untouched — the snapshot stays pre-stroke.
    pre: HashMap<TileCoord, Tile>,
}

impl OpenMaskStroke {
    fn new(layer_id: String) -> Self {
        Self {
            layer_id,
            pre: HashMap::new(),
        }
    }
}

/// Bounded undo/redo stack. The cap is enforced on every push by evicting
/// the *largest* committed entry until total bytes ≤ `cap_bytes`. Default
/// cap is [`History::DEFAULT_CAP_BYTES`].
pub struct History {
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    bytes: usize,
    cap_bytes: usize,
    open: Option<OpenStroke>,
    /// D10 / Sprint 17 (ADR-041) — open mask stroke (parallel to
    /// `open`, scoped to layer-mask edits).
    mask_open: Option<OpenMaskStroke>,
}

impl History {
    /// 100 MB — > 50 long strokes of headroom on a 16-SMU map plus
    /// hundreds of ProjectDiff entries.
    pub const DEFAULT_CAP_BYTES: usize = 100 * 1024 * 1024;

    pub fn new(cap_bytes: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            bytes: 0,
            cap_bytes,
            open: None,
            mask_open: None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn cap_bytes(&self) -> usize {
        self.cap_bytes
    }

    /// `true` between the first [`History::snapshot_rect`] of a stroke
    /// and the matching [`History::end_stroke`].
    pub fn stroke_open(&self) -> bool {
        self.open.is_some()
    }

    /// Drop the entire history AND any in-flight stroke. Called when the
    /// project state is replaced wholesale (procgen, load, new project).
    pub fn barrier(&mut self) {
        let had_open = self.open.is_some() || self.mask_open.is_some();
        if !self.undo.is_empty() || !self.redo.is_empty() || had_open {
            trace!(
                undo = self.undo.len(),
                redo = self.redo.len(),
                bytes = self.bytes,
                open_stroke = had_open,
                "undo barrier: clearing history"
            );
        }
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
        self.open = None;
        self.mask_open = None;
    }

    /// D10 / Sprint 17 (ADR-041) — capture the pre-stroke state of
    /// one tile. The first call per `(layer_id, coord)` stores
    /// `current_tile`; subsequent calls are no-ops. If `layer_id`
    /// switches mid-stroke (defensive — the UI doesn't allow it), the
    /// prior open stroke gets dropped with a `warn!` and a new one
    /// opens for the new layer.
    ///
    /// Callers (the app's `apply_mask_brush_at_elmos`) walk the
    /// brush bbox's tile coords via
    /// [`LayerMask::tile_coords_overlapping_rect`] and call this
    /// before invoking the brush.
    pub fn snapshot_mask_tile(&mut self, layer_id: &str, coord: TileCoord, current_tile: Tile) {
        match self.mask_open.as_mut() {
            Some(s) if s.layer_id == layer_id => {
                s.pre.entry(coord).or_insert(current_tile);
            }
            _ => {
                if self.mask_open.is_some() {
                    warn!(
                        layer_id,
                        "undo: mask stroke switched layers mid-stroke; dropping prior snapshot"
                    );
                }
                let mut stroke = OpenMaskStroke::new(layer_id.to_string());
                stroke.pre.insert(coord, current_tile);
                self.mask_open = Some(stroke);
            }
        }
    }

    /// D10 / Sprint 17 (ADR-041) — commit the in-flight mask stroke
    /// against `mask` (which is the live post-stroke layer mask).
    /// Builds a [`MaskEntry`] from the open snapshot's pre tiles +
    /// the current tile states, filters out tiles where before == after,
    /// pushes onto the undo stack. Returns `true` if anything was
    /// pushed (i.e. the stroke produced net change).
    pub fn end_mask_stroke(&mut self, mask: &LayerMask) -> bool {
        let Some(stroke) = self.mask_open.take() else {
            return false;
        };
        if stroke.pre.is_empty() {
            return false;
        }
        let tiles: Vec<(TileCoord, Tile, Tile)> = stroke
            .pre
            .into_iter()
            .filter_map(|(coord, before)| {
                let after = mask.clone_tile(coord);
                if before == after {
                    None
                } else {
                    Some((coord, before, after))
                }
            })
            .collect();
        if tiles.is_empty() {
            trace!(
                layer_id = %stroke.layer_id,
                "mask stroke committed no net change; discarding entry"
            );
            return false;
        }
        let entry = HistoryEntry::Mask(MaskEntry {
            layer_id: stroke.layer_id,
            tiles,
        });
        let added = entry.bytes();
        trace!(
            tiles = match &entry {
                HistoryEntry::Mask(m) => m.tiles.len(),
                _ => 0,
            },
            bytes = added,
            "mask stroke committed to undo history"
        );
        self.redo.clear();
        self.bytes += added;
        self.undo.push_back(entry);
        self.evict_until_under_cap();
        true
    }

    /// `true` while a mask stroke is in flight. Used by the app to
    /// decide whether `end_mask_stroke` would do anything on
    /// drag-stopped (it's safe to call unconditionally; this
    /// accessor exists for tests + telemetry).
    pub fn mask_stroke_open(&self) -> bool {
        self.mask_open.is_some()
    }

    /// D10 / Sprint 17 hotfix — `true` when the in-flight mask
    /// stroke already snapshotted `coord` for `layer_id`. The brush
    /// dispatch uses this to skip a redundant tile clone — promoted
    /// `Pixels` tiles clone to 64 KB each, so a continuous drag
    /// re-touching the same tiles every frame would churn ~15 MB/sec
    /// of redundant allocations at 60 FPS without this guard.
    pub fn mask_stroke_has_snapshot(&self, layer_id: &str, coord: TileCoord) -> bool {
        match self.mask_open.as_ref() {
            Some(s) if s.layer_id == layer_id => s.pre.contains_key(&coord),
            _ => false,
        }
    }

    // ───────── heightmap-stroke capture (unchanged from ADR-033) ─────────

    /// Capture pre-edit values for novel pixels in `rect`. Lazy: opens a
    /// stroke on the first call. Must be invoked **before** the brush
    /// writes to the heightmap so the captured values are pre-stroke.
    pub fn snapshot_rect(&mut self, hm: &Heightmap, rect: DirtyRect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        let stroke = match self.open.as_mut() {
            Some(s) if s.matches(hm) => s,
            Some(_) => {
                warn!("undo: heightmap dims changed mid-stroke; restarting stroke");
                self.open = Some(OpenStroke::new(hm.width(), hm.height()));
                self.open.as_mut().unwrap()
            }
            None => {
                self.open = Some(OpenStroke::new(hm.width(), hm.height()));
                self.open.as_mut().unwrap()
            }
        };
        stroke.snapshot_rect(hm, rect);
    }

    /// Commit the in-flight stroke. Builds one `Heightmap` entry covering
    /// the unioned bbox of snapshotted pixels and pushes it; no-op if no
    /// stroke is open or the bitset is empty.
    pub fn end_stroke(&mut self, hm: &Heightmap) {
        let Some(stroke) = self.open.take() else {
            return;
        };
        let Some(rect) = stroke.snapped_bbox() else {
            return;
        };
        let before = stroke.build_before(hm, rect);
        let entry = HistoryEntry::Heightmap(HeightmapEntry { rect, before });
        let added = entry.bytes();
        trace!(
            rect = ?(rect.x, rect.y, rect.w, rect.h),
            bytes = added,
            "stroke committed to undo history"
        );
        self.redo.clear();
        self.bytes = self.bytes.saturating_add(added);
        self.undo.push_back(entry);
        self.evict_until_under_cap();
    }

    // ───────── project-diff capture (B5) ─────────

    /// Push a project-level diff. Clears the redo stack (linear history,
    /// same as heightmap strokes) and enforces the cap.
    pub fn push_project_diff(&mut self, diff: ProjectDiff) {
        let entry = HistoryEntry::Project(diff);
        let added = entry.bytes();
        trace!(
            bytes = added,
            kind = ?std::mem::discriminant(&entry),
            "project diff committed to undo history"
        );
        self.redo.clear();
        self.bytes = self.bytes.saturating_add(added);
        self.undo.push_back(entry);
        self.evict_until_under_cap();
    }

    // ───────── pop / push dispatch ─────────

    /// Pop the top of the undo stack. Caller MUST follow up with
    /// [`History::push_to_redo`] using whatever entry restores the
    /// post-pop state — otherwise the redo stack drifts.
    pub fn pop_undo(&mut self) -> Option<HistoryEntry> {
        let entry = self.undo.pop_back()?;
        self.bytes = self.bytes.saturating_sub(entry.bytes());
        Some(entry)
    }

    /// Pop the top of the redo stack. Caller MUST follow up with
    /// [`History::push_to_undo`].
    pub fn pop_redo(&mut self) -> Option<HistoryEntry> {
        let entry = self.redo.pop()?;
        Some(entry)
    }

    /// Push onto the redo stack — used by `undo_one` after applying an
    /// entry, with the *inverse* of what was applied.
    pub fn push_to_redo(&mut self, entry: HistoryEntry) {
        self.redo.push(entry);
        // Redo entries don't accrue bytes against the cap. The cap
        // governs the undo stack; redo is bounded structurally by the
        // user's undo depth and gets cleared on the next push anyway.
    }

    /// Push onto the undo stack without clearing redo. Used by
    /// `redo_one` after applying an entry.
    pub fn push_to_undo(&mut self, entry: HistoryEntry) {
        let added = entry.bytes();
        self.bytes = self.bytes.saturating_add(added);
        self.undo.push_back(entry);
        self.evict_until_under_cap();
    }

    /// Apply the swap encoded by `entry` against `hm`. After this returns
    /// `entry` holds the *post-swap* pixels and can be pushed onto the
    /// opposite stack as the inverse. Returns the affected rect for the
    /// caller's GPU sub-upload.
    pub fn apply_heightmap(&self, entry: &mut HeightmapEntry, hm: &mut Heightmap) -> DirtyRect {
        hm.swap_rect(
            entry.rect.x,
            entry.rect.y,
            entry.rect.w,
            entry.rect.h,
            &mut entry.before,
        );
        entry.rect
    }

    // ───────── cap enforcement ─────────

    /// Evict the *largest* committed undo entry until total bytes are at
    /// or under cap. Largest-first (rather than oldest-first) keeps a
    /// single 2 MB stroke from evicting 20 kilobyte-sized ProjectDiff
    /// entries on every long sculpt.
    fn evict_until_under_cap(&mut self) {
        while self.bytes > self.cap_bytes && !self.undo.is_empty() {
            // VecDeque has no random-access remove that's both safe and
            // cheap; for the realistic 50-100 entry depth, a linear
            // scan is comfortably sub-microsecond.
            let mut largest_idx = 0;
            let mut largest_bytes = self.undo[0].bytes();
            for (i, e) in self.undo.iter().enumerate().skip(1) {
                let b = e.bytes();
                if b > largest_bytes {
                    largest_bytes = b;
                    largest_idx = i;
                }
            }
            let evicted = self
                .undo
                .remove(largest_idx)
                .expect("idx came from iter; must exist");
            self.bytes = self.bytes.saturating_sub(evicted.bytes());
            warn!(
                bytes_evicted = evicted.bytes(),
                bytes_remaining = self.bytes,
                cap_bytes = self.cap_bytes,
                evicted_idx = largest_idx,
                undo_depth = self.undo.len(),
                "undo: history over cap; evicted largest entry"
            );
        }
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAP_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    fn mk_hm() -> Heightmap {
        Heightmap::synth_ramp(MapSize::square(2)) // 129×129
    }

    // D10 / Sprint 17 (ADR-041) — mask undo tests.

    /// Helper: build a fresh 2-SMU LayerMask wrapped in a layer so the
    /// stroke tests can target it by id.
    fn make_test_mask() -> LayerMask {
        LayerMask::filled(MapSize::square(2), 0)
    }

    #[test]
    fn mask_undo_round_trip_restores_pre_stroke_state() {
        let mut mask = make_test_mask();
        let mut history = History::new(History::DEFAULT_CAP_BYTES);

        // Open a stroke: snapshot the tile that contains pixel (10, 10)
        // before mutating.
        let coord = TileCoord {
            tile_x: 0,
            tile_y: 0,
        };
        history.snapshot_mask_tile("layer-A", coord, mask.clone_tile(coord));
        assert!(history.mask_stroke_open());

        // Mutate.
        mask.set_pixel(10, 10, 200);
        assert_eq!(mask.sample(10, 10), 200);

        // Commit. Should push exactly one entry.
        let pushed = history.end_mask_stroke(&mask);
        assert!(pushed, "non-trivial stroke must commit an entry");
        assert_eq!(history.undo_depth(), 1);
        assert!(!history.mask_stroke_open());

        // Pop + apply inverse (the dispatcher path; we mirror it
        // inline for the test).
        let entry = history.pop_undo().unwrap();
        let HistoryEntry::Mask(mut mask_entry) = entry else {
            panic!("expected Mask entry");
        };
        for (c, before, after) in mask_entry.tiles.iter_mut() {
            let live = mask.clone_tile(*c);
            mask.restore_tile(*c, before.clone());
            *after = live;
        }
        assert_eq!(mask.sample(10, 10), 0, "undo must restore pre-stroke");
    }

    #[test]
    fn mask_undo_filters_no_op_strokes() {
        let mask = make_test_mask();
        let mut history = History::new(History::DEFAULT_CAP_BYTES);
        let coord = TileCoord {
            tile_x: 0,
            tile_y: 0,
        };
        // Snapshot the tile but never actually write to the mask.
        history.snapshot_mask_tile("layer-A", coord, mask.clone_tile(coord));
        let pushed = history.end_mask_stroke(&mask);
        assert!(!pushed, "stroke with no net change must NOT commit");
        assert_eq!(history.undo_depth(), 0);
    }

    #[test]
    fn mask_undo_bytes_accounting_includes_each_tile_snapshot() {
        let mut mask = make_test_mask();
        let mut history = History::new(History::DEFAULT_CAP_BYTES);
        let coord = TileCoord {
            tile_x: 0,
            tile_y: 0,
        };
        history.snapshot_mask_tile("layer-A", coord, mask.clone_tile(coord));
        mask.write_rect_with(0, 0, 256, 256, |_, _| 5);
        let pushed = history.end_mask_stroke(&mask);
        assert!(pushed);
        // The promoted Pixels tile is 64 KB; the snapshot stored a
        // Uniform(0) (24 B) before the write. Total bytes_account
        // should be >= TILE_PIXELS + small overhead.
        assert!(
            history.bytes() >= crate::TILE_PIXELS,
            "history.bytes() should reflect the 64 KB Pixels snapshot; got {}",
            history.bytes(),
        );
    }

    /// Helper: snapshot `rect`, then write `value` into the heightmap at
    /// every pixel of `rect`. Mirrors what `apply_brush_at` does in
    /// `main.rs`.
    fn write_stamp(history: &mut History, hm: &mut Heightmap, rect: DirtyRect, value: u16) {
        history.snapshot_rect(hm, rect);
        let stride = hm.width() as usize;
        for row in 0..rect.h {
            for col in 0..rect.w {
                let idx = ((rect.y + row) as usize) * stride + (rect.x + col) as usize;
                hm.data_mut()[idx] = value;
            }
        }
    }

    /// Walk the unified undo stack to completion, applying each entry
    /// against `hm` and the supplied group's positions. Used by the
    /// "stroke + 50 placements -> 51 walkback" test to mirror the
    /// app-side dispatcher without pulling main.rs into barme-core.
    fn drain_undo_to_state(history: &mut History, hm: &mut Heightmap, group: &mut AllyGroup) {
        while let Some(entry) = history.pop_undo() {
            match entry {
                HistoryEntry::Heightmap(mut e) => {
                    history.apply_heightmap(&mut e, hm);
                    history.push_to_redo(HistoryEntry::Heightmap(e));
                }
                HistoryEntry::Project(diff) => {
                    let inverse = apply_project_diff_for_test(diff, group);
                    history.push_to_redo(HistoryEntry::Project(inverse));
                }
                HistoryEntry::Mask(_) => {
                    // This test harness operates on heightmap + start
                    // positions only; mask entries aren't exercised
                    // here. The app-side dispatcher walks them via
                    // `LayerMask::restore_tile` (covered by the
                    // mask-undo unit tests below).
                }
            }
        }
    }

    /// Test-only project-diff dispatcher mirroring the app-side one.
    /// Operates on a single ally group's `start_positions`; the app
    /// dispatches by `ally_group_id` and forwards to the matching
    /// group in `Project.ally_groups`.
    fn apply_project_diff_for_test(diff: ProjectDiff, group: &mut AllyGroup) -> ProjectDiff {
        match diff {
            ProjectDiff::PlaceStartPosition { ally_group_id, pos } => {
                assert_eq!(ally_group_id, group.id);
                group.start_positions.retain(|q| *q != pos);
                ProjectDiff::DeleteStartPosition { ally_group_id, pos }
            }
            ProjectDiff::DeleteStartPosition { ally_group_id, pos } => {
                assert_eq!(ally_group_id, group.id);
                group.start_positions.push(pos);
                ProjectDiff::PlaceStartPosition { ally_group_id, pos }
            }
            ProjectDiff::MoveStartPosition {
                ally_group_id,
                from,
                to,
            } => {
                assert_eq!(ally_group_id, group.id);
                if let Some(p) = group.start_positions.iter_mut().find(|p| **p == to) {
                    *p = from;
                }
                ProjectDiff::MoveStartPosition {
                    ally_group_id,
                    from: to,
                    to: from,
                }
            }
            ProjectDiff::ApplyWizard(_) => unreachable!("not exercised in this test set"),
            // C4/C5/C9/D8 variants aren't dispatched against
            // AllyGroup — they target Project's metal_spots /
            // geo_vents / water / layers lists, which live outside
            // this helper's scope. The pinned unit tests for these
            // variants use the bytes() / history-stack checks
            // instead.
            ProjectDiff::PlaceMetalSpot { .. }
            | ProjectDiff::DeleteMetalSpot { .. }
            | ProjectDiff::MoveMetalSpot { .. }
            | ProjectDiff::SetExtractorRadius { .. }
            | ProjectDiff::PlaceGeoVent { .. }
            | ProjectDiff::DeleteGeoVent { .. }
            | ProjectDiff::MoveGeoVent { .. }
            | ProjectDiff::PlaceFeature { .. }
            | ProjectDiff::DeleteFeature { .. }
            | ProjectDiff::MoveFeature { .. }
            | ProjectDiff::SetWaterMode { .. }
            | ProjectDiff::EditWaterField { .. }
            | ProjectDiff::SetLavaAtmosphere { .. }
            | ProjectDiff::AddLayer { .. }
            | ProjectDiff::RemoveLayer { .. }
            | ProjectDiff::ReorderLayer { .. }
            | ProjectDiff::SetLayerProperty { .. }
            | ProjectDiff::EditMapInfo { .. } => {
                unreachable!(
                    "metal/geo/feature/water/layer/mapinfo diffs not exercised through AllyGroup test helper"
                )
            }
        }
    }

    #[test]
    fn round_trip_single_stamp() {
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();
        let mut h = History::default();

        let rect = DirtyRect {
            x: 10,
            y: 10,
            w: 5,
            h: 5,
        };
        write_stamp(&mut h, &mut hm, rect, 0xDEAD);
        h.end_stroke(&hm);
        assert!(h.can_undo());

        // Undo restores the original pixels.
        let mut entry = h.pop_undo().unwrap();
        let r = if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm)
        } else {
            panic!("expected Heightmap entry");
        };
        assert_eq!(r, rect);
        assert_eq!(hm.data(), &orig[..]);
        h.push_to_redo(entry);
        assert!(h.can_redo() && !h.can_undo());

        // Redo restores the edit.
        let mut entry = h.pop_redo().unwrap();
        let r2 = if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm)
        } else {
            panic!("expected Heightmap entry");
        };
        assert_eq!(r2, rect);
        let idx = (10 * hm.width() + 10) as usize;
        assert_eq!(hm.data()[idx], 0xDEAD);
        h.push_to_undo(entry);
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn overlapping_stamps_in_one_stroke_revert_to_pre_stroke_state() {
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();
        let mut h = History::default();

        let rect_a = DirtyRect {
            x: 10,
            y: 10,
            w: 4,
            h: 4,
        };
        let rect_b = DirtyRect {
            x: 12,
            y: 12,
            w: 4,
            h: 4,
        };
        write_stamp(&mut h, &mut hm, rect_a, 0xAAAA);
        write_stamp(&mut h, &mut hm, rect_b, 0xBBBB);
        h.end_stroke(&hm);

        let mut entry = h.pop_undo().unwrap();
        let r = if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm)
        } else {
            panic!("expected Heightmap entry");
        };
        // Bbox is the union: (10,10) to (15,15) inclusive → 6×6.
        assert_eq!(
            r,
            DirtyRect {
                x: 10,
                y: 10,
                w: 6,
                h: 6
            }
        );
        assert_eq!(hm.data(), &orig[..], "pre-stroke state must be restored");
    }

    #[test]
    fn snapshot_is_bounded_by_unioned_bbox_not_stamp_count() {
        let mut hm = mk_hm();
        let mut h = History::default();

        let rect = DirtyRect {
            x: 50,
            y: 50,
            w: 16,
            h: 16,
        };
        for _ in 0..120 {
            write_stamp(&mut h, &mut hm, rect, 0xFEED);
        }
        h.end_stroke(&hm);

        let entry = h.undo.back().unwrap();
        match entry {
            HistoryEntry::Heightmap(e) => {
                assert_eq!(e.rect, rect);
                assert_eq!(e.bytes(), 16 * 16 * 2);
            }
            _ => panic!("expected Heightmap entry"),
        }
    }

    #[test]
    fn snapshot_120_overlapping_stamps_stays_under_5x_affected_pixel_bytes() {
        let mut hm = Heightmap::synth_ramp(MapSize::square(4)); // 257×257
        let mut h = History::default();
        let stamp_w = 16u32;
        for i in 0..120u32 {
            let rect = DirtyRect {
                x: i,
                y: i,
                w: stamp_w,
                h: stamp_w,
            };
            write_stamp(&mut h, &mut hm, rect, 0xCAFE);
        }
        h.end_stroke(&hm);

        let entry = h.undo.back().unwrap();
        let HistoryEntry::Heightmap(e) = entry else {
            panic!("expected Heightmap entry")
        };
        let expected = (e.rect.w as usize) * (e.rect.h as usize) * 2;
        assert_eq!(e.bytes(), expected);
        assert!(e.bytes() < 1_000_000, "entry too large: {}", e.bytes());
    }

    #[test]
    fn snapshot_then_undo_byte_identical_to_pre_stroke() {
        let mut hm = Heightmap::synth_ramp(MapSize::square(2));
        let orig = hm.data().to_vec();
        let mut h = History::default();

        for (x, y, w, side) in [(5, 5, 8, 8u32), (20, 6, 4, 4), (8, 10, 6, 6)] {
            write_stamp(&mut h, &mut hm, DirtyRect { x, y, w, h: side }, 0xBEEF);
        }
        h.end_stroke(&hm);

        let mut entry = h.pop_undo().unwrap();
        if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm);
        }
        assert_eq!(hm.data(), &orig[..]);
    }

    #[test]
    fn new_heightmap_edit_clears_redo() {
        let mut hm = mk_hm();
        let mut h = History::default();
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            0x1111,
        );
        h.end_stroke(&hm);
        let mut e = h.pop_undo().unwrap();
        if let HistoryEntry::Heightmap(ref mut he) = e {
            h.apply_heightmap(he, &mut hm);
        }
        h.push_to_redo(e);
        assert!(h.can_redo());

        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 5,
                y: 5,
                w: 2,
                h: 2,
            },
            0x2222,
        );
        h.end_stroke(&hm);
        assert!(!h.can_redo(), "new push must clear redo stack");
    }

    #[test]
    fn new_project_diff_clears_redo() {
        let mut h = History::default();
        let pos = StartPosition {
            x_elmo: 100,
            z_elmo: 100,
        };
        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });
        let popped = h.pop_undo().unwrap();
        h.push_to_redo(popped);
        assert!(h.can_redo());

        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });
        assert!(!h.can_redo(), "new project-diff push must clear redo stack");
    }

    #[test]
    fn barrier_clears_everything_including_open_stroke() {
        let mut hm = mk_hm();
        let mut h = History::default();
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            0x3333,
        );
        h.end_stroke(&hm);
        let mut e = h.pop_undo().unwrap();
        if let HistoryEntry::Heightmap(ref mut he) = e {
            h.apply_heightmap(he, &mut hm);
        }
        h.push_to_redo(e);

        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos: StartPosition {
                x_elmo: 50,
                z_elmo: 50,
            },
        });

        // Open a fresh stroke without committing.
        h.snapshot_rect(
            &hm,
            DirtyRect {
                x: 8,
                y: 8,
                w: 2,
                h: 2,
            },
        );
        assert!(h.stroke_open());

        h.barrier();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
        assert!(!h.stroke_open(), "barrier must drop the open stroke");
        assert_eq!(h.bytes(), 0);
    }

    #[test]
    fn end_stroke_with_no_snapshots_is_noop() {
        let hm = mk_hm();
        let mut h = History::default();
        h.end_stroke(&hm);
        assert!(!h.can_undo());

        h.snapshot_rect(
            &hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
        );
        h.end_stroke(&hm);
        assert!(!h.can_undo());
    }

    #[test]
    fn cap_evicts_largest_not_oldest() {
        // Push: small project diff (oldest), then a 'large' stroke.
        // Pre-B5 the oldest (the diff) would have been evicted; B5
        // evicts the largest (the stroke).
        //
        // `ProjectDiff::bytes()` reports the full enum inline size
        // (constant across variants — std::mem::size_of::<Self>()),
        // while `HeightmapEntry::bytes()` reports only the heap cost
        // of `before`. So a "larger" stamp needs heap bytes >
        // ProjectDiff inline. C4/C5/C6 widened the enum to ~88 bytes
        // inline (`MoveFeature { from: FeatureInstance, to: FeatureInstance }`
        // is the load-bearing variant). A 16×16 stamp = 256 pixels ×
        // 2 = 512 bytes of heap — unambiguously larger; the test
        // stays robust against future inline-size growth.
        let mut hm = mk_hm();
        let diff_inline = std::mem::size_of::<ProjectDiff>();
        let cap = diff_inline * 4; // comfortably holds one diff; smaller than heap-cost of the stamp.
        let mut h = History::new(cap);

        let pos = StartPosition {
            x_elmo: 1,
            z_elmo: 1,
        };
        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });
        let small_bytes = h.bytes();

        // Now push a heightmap entry whose heap cost (512 bytes)
        // exceeds the ProjectDiff inline + cap.
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 16,
                h: 16,
            },
            0xF00D,
        );
        h.end_stroke(&hm);

        // Eviction should kick out the largest (the heightmap entry) —
        // the small ProjectDiff survives.
        assert!(h.bytes() <= cap);
        assert_eq!(h.undo_depth(), 1);
        let surviving = h.undo.back().unwrap();
        match surviving {
            HistoryEntry::Project(ProjectDiff::PlaceStartPosition {
                ally_group_id,
                pos: p2,
            }) => {
                assert_eq!(*ally_group_id, 0);
                assert_eq!(p2.x_elmo, 1);
            }
            _ => panic!("largest entry should have been evicted, got {surviving:?}"),
        }
        let _ = small_bytes;
    }

    #[test]
    fn redo_after_undo_chain_heightmap() {
        let mut hm = mk_hm();
        let mut h = History::default();

        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            0x1111,
        );
        h.end_stroke(&hm);
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 4,
                y: 4,
                w: 4,
                h: 4,
            },
            0x2222,
        );
        h.end_stroke(&hm);

        let final_state = hm.data().to_vec();
        // Undo twice.
        for _ in 0..2 {
            let mut e = h.pop_undo().unwrap();
            if let HistoryEntry::Heightmap(ref mut he) = e {
                h.apply_heightmap(he, &mut hm);
            }
            h.push_to_redo(e);
        }
        // Redo twice.
        for _ in 0..2 {
            let mut e = h.pop_redo().unwrap();
            if let HistoryEntry::Heightmap(ref mut he) = e {
                h.apply_heightmap(he, &mut hm);
            }
            h.push_to_undo(e);
        }
        assert_eq!(hm.data(), &final_state[..]);
    }

    // ─────── B5 project-diff tests ───────

    #[test]
    fn place_start_position_round_trip() {
        let mut h = History::default();
        let mut group = AllyGroup::new(0);
        let pos = StartPosition {
            x_elmo: 100,
            z_elmo: 200,
        };
        group.start_positions.push(pos);
        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });

        // Undo: remove pos.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            panic!("expected Project entry");
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        h.push_to_redo(HistoryEntry::Project(inverse));
        assert!(
            group.start_positions.is_empty(),
            "undo should remove the placed position"
        );

        // Redo: re-place.
        let entry = h.pop_redo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            panic!("expected Project entry");
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        h.push_to_undo(HistoryEntry::Project(inverse));
        assert_eq!(group.start_positions.len(), 1);
        assert_eq!(group.start_positions[0], pos);
    }

    #[test]
    fn move_start_position_round_trip() {
        let mut h = History::default();
        let from = StartPosition {
            x_elmo: 500,
            z_elmo: 600,
        };
        let to = StartPosition {
            x_elmo: 1500,
            z_elmo: 1600,
        };
        let mut group = AllyGroup {
            start_positions: vec![to],
            ..AllyGroup::new(0)
        };
        // App pretends the marker moved from `from` to `to`.
        h.push_project_diff(ProjectDiff::MoveStartPosition {
            ally_group_id: 0,
            from,
            to,
        });

        // Undo.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        assert_eq!(group.start_positions[0], from);
        h.push_to_redo(HistoryEntry::Project(inverse));

        // Redo.
        let entry = h.pop_redo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        apply_project_diff_for_test(diff, &mut group);
        assert_eq!(group.start_positions[0], to);
    }

    #[test]
    fn delete_start_position_round_trip() {
        let mut h = History::default();
        let pos = StartPosition {
            x_elmo: 99,
            z_elmo: 99,
        };
        let mut group = AllyGroup::new(0);
        // App removed pos already; record the diff.
        h.push_project_diff(ProjectDiff::DeleteStartPosition {
            ally_group_id: 0,
            pos,
        });

        // Undo: pos returns.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        assert_eq!(group.start_positions.len(), 1);
        h.push_to_redo(HistoryEntry::Project(inverse));

        // Redo: pos goes away again.
        let entry = h.pop_redo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        apply_project_diff_for_test(diff, &mut group);
        assert!(group.start_positions.is_empty());
    }

    /// Phase-3-plan smoke: one long stroke + 50 placements + Ctrl-Z×51
    /// returns to pre-stroke state.
    #[test]
    fn stroke_then_50_placements_walks_back_in_51_steps() {
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();
        let mut group = AllyGroup::new(0);
        let mut h = History::default();

        // 1. Long stroke — 80 overlapping stamps along a diagonal.
        for i in 0..80u32 {
            write_stamp(
                &mut h,
                &mut hm,
                DirtyRect {
                    x: i,
                    y: i,
                    w: 8,
                    h: 8,
                },
                0xBEEF,
            );
        }
        h.end_stroke(&hm);
        assert_eq!(h.undo_depth(), 1);

        // 2. 50 start-position placements.
        for i in 0..50i32 {
            let pos = StartPosition {
                x_elmo: i * 10,
                z_elmo: i * 10,
            };
            group.start_positions.push(pos);
            h.push_project_diff(ProjectDiff::PlaceStartPosition {
                ally_group_id: 0,
                pos,
            });
        }
        assert_eq!(h.undo_depth(), 51);
        assert_eq!(group.start_positions.len(), 50);

        // 3. Walk all 51 back. After each pop the heightmap or position
        //    list mutates. After 51 pops we expect pre-stroke state.
        drain_undo_to_state(&mut h, &mut hm, &mut group);

        assert!(
            group.start_positions.is_empty(),
            "all 50 positions should be cleared"
        );
        assert_eq!(hm.data(), &orig[..], "heightmap should be pre-stroke");
        assert!(!h.can_undo(), "stack should be empty");
        assert_eq!(h.bytes(), 0);
        assert_eq!(h.redo_depth(), 51, "all 51 entries on the redo side");
    }

    #[test]
    fn apply_wizard_snapshot_round_trip() {
        let mut h = History::default();
        let mut g = AllyGroup::new(0);
        g.start_positions.push(StartPosition {
            x_elmo: 1,
            z_elmo: 1,
        });
        let snap = WizardSnapshot {
            project_name: "pre-wizard".to_string(),
            map_size: MapSize::square(8),
            height_scale: 256.0,
            symmetry: SymmetryAxis::None,
            rotational_fold: 2,
            ally_groups: vec![g],
            procgen_expr: "x".to_string(),
            procgen_domain: Domain::Unit,
        };
        h.push_project_diff(ProjectDiff::ApplyWizard(Box::new(snap.clone())));
        assert_eq!(h.undo_depth(), 1);

        // Pop and inspect.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(ProjectDiff::ApplyWizard(boxed)) = entry else {
            panic!("expected ApplyWizard");
        };
        assert_eq!(*boxed, snap);
    }

    /// C4 (Sprint 11): metal-spot diffs are inline (no heap),
    /// matching `PlaceStartPosition`. Verify they account for bytes
    /// the same way under cap pressure.
    #[test]
    fn metal_spot_diff_bytes_match_inline_enum_size() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let place = ProjectDiff::PlaceMetalSpot {
            spot: MetalSpot::new(100, 200),
        };
        let delete = ProjectDiff::DeleteMetalSpot {
            spot: MetalSpot::new(100, 200),
        };
        let m = ProjectDiff::MoveMetalSpot {
            from: MetalSpot::new(100, 100),
            to: MetalSpot::new(200, 200),
        };
        assert_eq!(place.bytes(), inline);
        assert_eq!(delete.bytes(), inline);
        assert_eq!(m.bytes(), inline);
    }

    /// Same for geo-vent diffs.
    #[test]
    fn geo_vent_diff_bytes_match_inline_enum_size() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let place = ProjectDiff::PlaceGeoVent {
            vent: GeoVent::new(100, 200),
        };
        let delete = ProjectDiff::DeleteGeoVent {
            vent: GeoVent::new(100, 200),
        };
        let m = ProjectDiff::MoveGeoVent {
            from: GeoVent::new(100, 100),
            to: GeoVent::new(200, 200),
        };
        assert_eq!(place.bytes(), inline);
        assert_eq!(delete.bytes(), inline);
        assert_eq!(m.bytes(), inline);
    }

    /// C6 (Sprint 12): `FeatureInstance` carries a heap-allocated
    /// `String name`, so the bytes accounting must include its
    /// capacity in addition to the enum inline size. This keeps the
    /// 100 MB cap honest under feature-heavy workloads (a placed
    /// pinetree's name is 8 bytes of heap, an agorm_talltree6 is 15).
    #[test]
    fn feature_diff_bytes_includes_name_capacity() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let f = FeatureInstance::new("agorm_talltree6", 1024, 2048, 0);
        let cap = f.name.capacity();
        assert!(cap >= "agorm_talltree6".len(), "string capacity sanity");
        let place = ProjectDiff::PlaceFeature { feature: f.clone() };
        let delete = ProjectDiff::DeleteFeature { feature: f.clone() };
        let mv = ProjectDiff::MoveFeature {
            from: f.clone(),
            to: FeatureInstance::new("agorm_talltree6", 2048, 2048, 16384),
        };
        assert_eq!(place.bytes(), inline + cap);
        assert_eq!(delete.bytes(), inline + cap);
        // Move pays for both from + to names.
        let to_cap = match &mv {
            ProjectDiff::MoveFeature { to, .. } => to.name.capacity(),
            _ => unreachable!(),
        };
        assert_eq!(mv.bytes(), inline + cap + to_cap);
    }

    /// Setting extractor_radius is also bytes-inline.
    #[test]
    fn extractor_radius_diff_is_inline() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let d = ProjectDiff::SetExtractorRadius {
            from: 80.0,
            to: 120.0,
        };
        assert_eq!(d.bytes(), inline);
    }

    /// C9 / Sprint 14: water diffs are inline — neither variant
    /// carries a heap allocation, matching the metal/geo pattern.
    #[test]
    fn water_diffs_are_inline() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let sm = ProjectDiff::SetWaterMode {
            from: WaterMode::None,
            to: WaterMode::Ocean,
        };
        let ef = ProjectDiff::EditWaterField {
            field: WaterField::Damage,
            from: WaterValue::Float(None),
            to: WaterValue::Float(Some(200.0)),
        };
        assert_eq!(sm.bytes(), inline);
        assert_eq!(ef.bytes(), inline);
    }

    /// C9 / Sprint 14: SetWaterMode pushes/pops through history like
    /// any other ProjectDiff. The actual project-side mutation lives
    /// in `App::apply_project_diff` (main.rs); the core crate's
    /// contract is just stack accounting + redo clearance.
    #[test]
    fn set_water_mode_clears_redo_and_grows_undo() {
        let mut h = History::default();
        h.push_project_diff(ProjectDiff::SetWaterMode {
            from: WaterMode::None,
            to: WaterMode::Ocean,
        });
        let popped = h.pop_undo().unwrap();
        h.push_to_redo(popped);
        assert!(h.can_redo());
        // A new push of any kind clears redo.
        h.push_project_diff(ProjectDiff::EditWaterField {
            field: WaterField::SurfaceAlpha,
            from: WaterValue::Float(None),
            to: WaterValue::Float(Some(0.4)),
        });
        assert!(!h.can_redo());
    }

    /// D8 / Sprint 15 (ADR-038): `ReorderLayer` is inline; `AddLayer`
    /// and `RemoveLayer` carry the mask bytes (capacity) plus the
    /// id+name string capacities so the cap eviction stays honest.
    /// The expected size is computed from the cloned-into-Box
    /// `TextureLayer` (not the source layer) — `String::clone` may
    /// shrink capacity, so reading the pre-clone capacities would
    /// over-count by a handful of bytes.
    #[test]
    fn layer_diffs_bytes_account_for_mask_and_strings() {
        let inline = std::mem::size_of::<ProjectDiff>();

        let reorder = ProjectDiff::ReorderLayer { from: 0, to: 1 };
        assert_eq!(reorder.bytes(), inline);

        let layer = crate::layers::TextureLayer::new(
            crate::layers::LayerSource::Slot { id: 3 },
            crate::MapSize::square(2),
            0,
        );

        let add = ProjectDiff::AddLayer {
            index: 0,
            layer: Box::new(layer.clone()),
        };
        let ProjectDiff::AddLayer {
            layer: ref add_l, ..
        } = add
        else {
            unreachable!()
        };
        let expected_add =
            inline + add_l.mask.resident_bytes() + add_l.id.capacity() + add_l.name.capacity();
        assert_eq!(add.bytes(), expected_add);

        let rm = ProjectDiff::RemoveLayer {
            index: 0,
            layer: Box::new(layer),
        };
        let ProjectDiff::RemoveLayer {
            layer: ref rm_l, ..
        } = rm
        else {
            unreachable!()
        };
        let expected_rm =
            inline + rm_l.mask.resident_bytes() + rm_l.id.capacity() + rm_l.name.capacity();
        assert_eq!(rm.bytes(), expected_rm);
    }

    /// `SetLayerProperty` round-trips through the history stack and
    /// clears redo just like every other ProjectDiff variant.
    #[test]
    fn set_layer_property_pushes_through_history_and_clears_redo() {
        let mut h = History::default();
        h.push_project_diff(ProjectDiff::SetLayerProperty {
            layer_id: "abc".to_string(),
            from: LayerPropertyValue::Visible(true),
            to: LayerPropertyValue::Visible(false),
        });
        let popped = h.pop_undo().unwrap();
        h.push_to_redo(popped);
        assert!(h.can_redo());

        // A new push of any kind clears redo.
        h.push_project_diff(ProjectDiff::ReorderLayer { from: 0, to: 1 });
        assert!(!h.can_redo());
    }

    /// `AddLayer` of a fully-painted 16-SMU mask = 64 MB worth of
    /// concrete tiles. The cap eviction must kick out the mask-heavy
    /// entry when smaller ProjectDiffs share the stack, matching the
    /// `cap_evicts_largest_not_oldest` shape of the heightmap path.
    ///
    /// Sprint 16 (tiled COW): an uniform-fill mask is ~1 KB resident
    /// (only the tile metadata + version array). To exercise the
    /// eviction path, force every tile into `Pixels` by painting a
    /// stamp that wraps the whole mask before snapshotting the layer.
    #[test]
    fn add_layer_with_big_mask_is_largest_and_gets_evicted_first() {
        let mut layer = crate::layers::TextureLayer::new(
            crate::layers::LayerSource::Slot { id: 0 },
            crate::MapSize::square(4),
            255,
        );
        // Force every tile to materialise as `Pixels` so the mask
        // actually consumes its full byte budget. A single stamp
        // that covers the whole mask + a non-identity value writes
        // every tile through `write_rect_with`, promoting them all.
        let (w, h) = (layer.mask.width, layer.mask.height);
        layer.mask.write_rect_with(0, 0, w, h, |_, _| 200).unwrap();
        let mask_bytes = layer.mask.resident_bytes();
        // Sanity: every tile promoted → cost should be at least the
        // raw pixel count.
        assert!(
            mask_bytes >= (w as usize) * (h as usize),
            "expected fully-painted mask >= {} bytes, got {mask_bytes}",
            (w as usize) * (h as usize)
        );

        // Cap small enough to evict the layer but not the four
        // ReorderLayer entries (which are inline-sized).
        let cap = mask_bytes / 2;
        let mut h = History::new(cap);
        for i in 0..4 {
            h.push_project_diff(ProjectDiff::ReorderLayer { from: i, to: i + 1 });
        }
        let small_depth = h.undo_depth();

        h.push_project_diff(ProjectDiff::AddLayer {
            index: 0,
            layer: Box::new(layer),
        });
        // The layer entry exceeded cap on its own; eviction kicks
        // largest (the AddLayer) before the inline-sized reorders.
        assert_eq!(h.undo_depth(), small_depth, "all reorders survive");
        for e in h.undo.iter() {
            assert!(matches!(
                e,
                HistoryEntry::Project(ProjectDiff::ReorderLayer { .. })
            ));
        }
    }

    /// C9 / Sprint 14 (Slice 4): SetLavaAtmosphere is inline and
    /// pushes through history like other water diffs.
    #[test]
    fn set_lava_atmosphere_diff_is_inline_and_round_trips_through_history() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let d = ProjectDiff::SetLavaAtmosphere {
            from: false,
            to: true,
        };
        assert_eq!(d.bytes(), inline);

        let mut h = History::default();
        h.push_project_diff(d);
        let popped = h.pop_undo().unwrap();
        match popped {
            HistoryEntry::Project(ProjectDiff::SetLavaAtmosphere { from, to }) => {
                assert!(!from);
                assert!(to);
            }
            other => panic!("expected SetLavaAtmosphere, got {other:?}"),
        }
    }

    /// C4/C5 diff variants push through history just like the F8
    /// variants — sanity check the stack accounting and redo
    /// clearance on a metal-spot place.
    #[test]
    fn place_metal_spot_clears_redo_and_grows_undo() {
        let mut h = History::default();
        let spot = MetalSpot::new(1, 2);
        h.push_project_diff(ProjectDiff::PlaceMetalSpot { spot });
        let popped = h.pop_undo().unwrap();
        h.push_to_redo(popped);
        assert!(h.can_redo());

        // A new push must clear redo, same as the F8 path.
        h.push_project_diff(ProjectDiff::PlaceGeoVent {
            vent: GeoVent::new(3, 4),
        });
        assert!(!h.can_redo());
    }

    #[test]
    fn cap_eviction_evicts_largest_under_pressure() {
        // 100 small project diffs followed by one huge heightmap stroke.
        // Cap is tight enough to force eviction; we should lose the
        // heightmap entry, not the small diffs.
        let mut hm = mk_hm();
        let mut h = History::new(10_000);
        for i in 0..100i32 {
            h.push_project_diff(ProjectDiff::PlaceStartPosition {
                ally_group_id: 0,
                pos: StartPosition {
                    x_elmo: i,
                    z_elmo: i,
                },
            });
        }
        let depth_before = h.undo_depth();
        // ~50×50 stamp = 50*50*2 = 5000 bytes. Push another 5000-byte
        // entry: total would be 10000 + diffs (~few kb). Push a 6000-
        // byte stroke to force eviction.
        let mut h2 = h;
        write_stamp(
            &mut h2,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 80,
                h: 80,
            },
            0xF00D,
        );
        h2.end_stroke(&hm);
        // The heightmap entry is the largest by far (80*80*2 = 12800).
        // If 12800 > cap by itself, ALL entries evict — but our diffs
        // are much smaller (~32 bytes each), so the eviction should
        // first pop the heightmap entry alone.
        assert!(h2.bytes() <= h2.cap_bytes());
        assert_eq!(h2.undo_depth(), depth_before, "all small diffs survive");
        for e in h2.undo.iter() {
            assert!(matches!(e, HistoryEntry::Project(_)));
        }
    }
}
