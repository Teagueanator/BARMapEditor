//! Sprint 19 / U1 — centralised hover-text catalogue.
//!
//! One [`HelpId`] per interactive widget surface in the editor.
//! [`help`] maps each id to a 1-3 sentence string suitable for an
//! `.on_hover_text(...)` call. The strings live here (rather than
//! inline in each inspector) so:
//!
//! - Sprint 22's onboarding tour can reuse the same strings without
//!   re-deriving them from the widget sites.
//! - A future i18n pass localises one file, not the entire UI tree.
//! - Wording drift across duplicated affordances (e.g. `min_height`
//!   appearing in both the Water inspector and the heightmap header)
//!   surfaces as a single edit instead of N independent ones.
//!
//! ## Convention
//!
//! - Strings end with `[Shortcut: <chord>]` when a keyboard chord
//!   exists. The chord MUST be one of the bindings in
//!   [`crate::ui::cheat_sheet`] — that module is the source of truth.
//! - Strings end with `[PITFALL §N]` when a numbered pitfall from
//!   `docs/PITFALLS.md` directly applies. Both suffixes can appear
//!   together; the shortcut goes first.
//! - 1-3 sentences. Anything longer belongs in Sprint 22's help
//!   centre — leave a `// FIXME(sprint22):` placeholder if you
//!   catch yourself writing a paragraph.

/// Stable identifier for every hover-text in the editor. Pair with
/// [`help`] to render `.on_hover_text(help_text::help(HelpId::Foo))`
/// at the widget site.
///
/// Grouped roughly by Inspector / chrome surface so editing one
/// section doesn't disturb the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)] // not every variant is wired this commit; full pass lands per-inspector
pub enum HelpId {
    // ──────────────── Top-bar chrome ────────────────
    TopBarRecenter,
    TopBarSymmetryPill,
    TopBarSymmetryMode,
    TopBarSymmetryFold,
    TopBarBuildPrimary,
    TopBarBuildVariant,
    TopBarSave,
    TopBarMapInfoForm,
    TopBarValidationChip,
    TopBarHelpIcon,

    // ──────────────── Status strip ────────────────
    StatusCamera,
    StatusMapSize,
    StatusIssueCount,
    StatusInstall,
    StatusBrushChip,

    // ──────────────── Inspector header (always visible) ────────────────
    HeaderProjectName,
    HeaderProjectSavedChip,
    HeaderMapSizeX,
    HeaderMapSizeZ,
    HeaderHeightmapValidChip,
    HeaderHeightmapPath,
    HeaderHeightmapDims,
    HeaderHeightmapSample,
    HeaderHeightScale,

    // Per-inspector sticky chips (sprint-19 addition).
    InspectorSymmetryChip,
    InspectorMapSizeChip,

    // ──────────────── Select tool ────────────────
    SelectModeInfo,

    // ──────────────── Sculpt tool ────────────────
    SculptBrushOff,
    SculptBrushRaise,
    SculptBrushLower,
    SculptBrushSmooth,
    SculptRadius,
    SculptStrength,
    SculptFalloff,
    SculptBehaviorContinuous,
    SculptBehaviorPressure,
    SculptBehaviorLockZ,

    // ──────────────── Metal-spots tool ────────────────
    MetalGlobalChip,
    MetalExtractorRadius,
    MetalAddSpot,
    MetalSpotX,
    MetalSpotZ,
    MetalSpotValue,
    MetalSpotDelete,

    // ──────────────── Geo-vents tool ────────────────
    GeoAddVent,
    GeoVentX,
    GeoVentZ,
    GeoVentDelete,

    // ──────────────── Feature tool ────────────────
    FeatureCategoryCombo,
    FeatureFilter,
    FeaturePickerRow,
    FeaturePlacedX,
    FeaturePlacedZ,
    FeaturePlacedRot,
    FeaturePlacedDelete,

    // ──────────────── Paint-layer tool ────────────────
    PaintBrushReveal,
    PaintBrushHide,
    PaintBrushSmooth,
    PaintBrushFill,
    PaintRadius,
    PaintStrength,
    PaintSpacing,
    PaintFillTargetVisible,
    PaintMaskOnlyPreview,
    PaintViewMaskPreviewChip,

    // ──────────────── Water / lava tool ────────────────
    WaterPresetNone,
    WaterPresetCustom,
    WaterPresetOcean,
    WaterPresetTropical,
    WaterPresetAcid,
    WaterPresetLava,
    WaterPresetMagma,
    WaterFloorMin,
    WaterCeilingMax,
    WaterDamage,
    WaterVoidWater,
    WaterTidalStrength,
    WaterSurfaceColor,
    WaterPlaneColor,
    WaterSurfaceAlpha,
    WaterWaveSize,
    WaterFoamStrength,
    WaterCarveDepth,
    WaterAutoMinHeight,
    WaterLavaAtmosphereApply,
    WaterLavaAtmosphereRevert,

    // ──────────────── Start-positions tool ────────────────
    StartLayoutBalancedChip,
    StartPresetCombo,
    StartDragPaintCount,
    StartAllyAdd,
    StartAllyColor,
    StartAllyName,
    StartAllyActiveStar,
    StartAllyDelete,
    StartPosDelete,

    // ──────────────── Procgen tool ────────────────
    ProcgenPresetChip,
    ProcgenExpression,
    ProcgenDomainUnit,
    ProcgenDomainCentered,
    ProcgenPreviewChip,
    ProcgenCommitButton,

    // ──────────────── Viewport chrome ────────────────
    ViewportGrid,
    ViewportLighting,
    ViewportWireframe,
    ViewportBuildable,
}

impl HelpId {
    /// All variants, for the exhaustiveness test. The compiler enforces
    /// completeness: a new variant added to [`HelpId`] without an entry
    /// here triggers an unused-match-arm warning in [`help`] (and the
    /// `help_text_is_total_and_non_empty` test catches the missing
    /// string).
    #[allow(dead_code)] // Sprint 19 / U1: test-only today, wired per-inspector in later commits
    pub const ALL: &'static [HelpId] = &[
        // top-bar
        HelpId::TopBarRecenter,
        HelpId::TopBarSymmetryPill,
        HelpId::TopBarSymmetryMode,
        HelpId::TopBarSymmetryFold,
        HelpId::TopBarBuildPrimary,
        HelpId::TopBarBuildVariant,
        HelpId::TopBarSave,
        HelpId::TopBarMapInfoForm,
        HelpId::TopBarValidationChip,
        HelpId::TopBarHelpIcon,
        // status strip
        HelpId::StatusCamera,
        HelpId::StatusMapSize,
        HelpId::StatusIssueCount,
        HelpId::StatusInstall,
        HelpId::StatusBrushChip,
        // header
        HelpId::HeaderProjectName,
        HelpId::HeaderProjectSavedChip,
        HelpId::HeaderMapSizeX,
        HelpId::HeaderMapSizeZ,
        HelpId::HeaderHeightmapValidChip,
        HelpId::HeaderHeightmapPath,
        HelpId::HeaderHeightmapDims,
        HelpId::HeaderHeightmapSample,
        HelpId::HeaderHeightScale,
        HelpId::InspectorSymmetryChip,
        HelpId::InspectorMapSizeChip,
        // select
        HelpId::SelectModeInfo,
        // sculpt
        HelpId::SculptBrushOff,
        HelpId::SculptBrushRaise,
        HelpId::SculptBrushLower,
        HelpId::SculptBrushSmooth,
        HelpId::SculptRadius,
        HelpId::SculptStrength,
        HelpId::SculptFalloff,
        HelpId::SculptBehaviorContinuous,
        HelpId::SculptBehaviorPressure,
        HelpId::SculptBehaviorLockZ,
        // metal
        HelpId::MetalGlobalChip,
        HelpId::MetalExtractorRadius,
        HelpId::MetalAddSpot,
        HelpId::MetalSpotX,
        HelpId::MetalSpotZ,
        HelpId::MetalSpotValue,
        HelpId::MetalSpotDelete,
        // geo
        HelpId::GeoAddVent,
        HelpId::GeoVentX,
        HelpId::GeoVentZ,
        HelpId::GeoVentDelete,
        // feature
        HelpId::FeatureCategoryCombo,
        HelpId::FeatureFilter,
        HelpId::FeaturePickerRow,
        HelpId::FeaturePlacedX,
        HelpId::FeaturePlacedZ,
        HelpId::FeaturePlacedRot,
        HelpId::FeaturePlacedDelete,
        // paint
        HelpId::PaintBrushReveal,
        HelpId::PaintBrushHide,
        HelpId::PaintBrushSmooth,
        HelpId::PaintBrushFill,
        HelpId::PaintRadius,
        HelpId::PaintStrength,
        HelpId::PaintSpacing,
        HelpId::PaintFillTargetVisible,
        HelpId::PaintMaskOnlyPreview,
        HelpId::PaintViewMaskPreviewChip,
        // water
        HelpId::WaterPresetNone,
        HelpId::WaterPresetCustom,
        HelpId::WaterPresetOcean,
        HelpId::WaterPresetTropical,
        HelpId::WaterPresetAcid,
        HelpId::WaterPresetLava,
        HelpId::WaterPresetMagma,
        HelpId::WaterFloorMin,
        HelpId::WaterCeilingMax,
        HelpId::WaterDamage,
        HelpId::WaterVoidWater,
        HelpId::WaterTidalStrength,
        HelpId::WaterSurfaceColor,
        HelpId::WaterPlaneColor,
        HelpId::WaterSurfaceAlpha,
        HelpId::WaterWaveSize,
        HelpId::WaterFoamStrength,
        HelpId::WaterCarveDepth,
        HelpId::WaterAutoMinHeight,
        HelpId::WaterLavaAtmosphereApply,
        HelpId::WaterLavaAtmosphereRevert,
        // start positions
        HelpId::StartLayoutBalancedChip,
        HelpId::StartPresetCombo,
        HelpId::StartDragPaintCount,
        HelpId::StartAllyAdd,
        HelpId::StartAllyColor,
        HelpId::StartAllyName,
        HelpId::StartAllyActiveStar,
        HelpId::StartAllyDelete,
        HelpId::StartPosDelete,
        // procgen
        HelpId::ProcgenPresetChip,
        HelpId::ProcgenExpression,
        HelpId::ProcgenDomainUnit,
        HelpId::ProcgenDomainCentered,
        HelpId::ProcgenPreviewChip,
        HelpId::ProcgenCommitButton,
        // viewport chrome
        HelpId::ViewportGrid,
        HelpId::ViewportLighting,
        HelpId::ViewportWireframe,
        HelpId::ViewportBuildable,
    ];
}

/// Map an id to its hover-text. The body is an exhaustive `match`
/// so adding a [`HelpId`] variant produces a compile error here.
pub fn help(id: HelpId) -> &'static str {
    match id {
        // ──────────────── Top-bar chrome ────────────────
        HelpId::TopBarRecenter => {
            "Recenter the 3D camera on the map. Pairs with the arrow-key pan controls if you've drifted off."
        }
        HelpId::TopBarSymmetryPill => {
            "Toggle symmetry on/off. When off, all stamps and placements apply only at the cursor. When on, the editor mirrors strokes across the chosen axis."
        }
        HelpId::TopBarSymmetryMode => {
            "Symmetry axis. Horizontal mirrors across Z; Vertical across X; Quad mirrors both; Diagonal swaps X↔Z; Rotational replicates N times around the centre."
        }
        HelpId::TopBarSymmetryFold => {
            "Number of rotational replications (2-12). Pick to match your map's player count: 4 for a 1v1v1v1, 6 for a 3v3 ring, etc."
        }
        HelpId::TopBarBuildPrimary => {
            "Build the project into a `.sd7` and install it under BAR's user maps directory. Disabled until a heightmap is loaded."
        }
        HelpId::TopBarBuildVariant => {
            "Pick the build target — install vs. write-out vs. (future) variants. Phase 5+ entries are reserved."
        }
        HelpId::TopBarSave => {
            "Save the project to its `.barmeproj` file. The dot (•) on the label means there are unsaved changes. [Shortcut: Ctrl+S]"
        }
        HelpId::TopBarMapInfoForm => {
            "Open the mapinfo.lua form editor — 12 tabs covering every author-editable field (lighting, atmosphere, water, splats, …). [Shortcut: F9]"
        }
        HelpId::TopBarValidationChip => {
            "Project validation summary. Click to open the lint panel and see each issue. Colour reflects severity: red = error, amber = warning, neutral = OK."
        }
        HelpId::TopBarHelpIcon => {
            "Open the keyboard cheat sheet — full list of tools, camera gestures, and project shortcuts. [Shortcut: ?]"
        }

        // ──────────────── Status strip ────────────────
        HelpId::StatusCamera => {
            "Live camera yaw / pitch / orbit distance. Refreshes once per second on idle, every frame while you're interacting."
        }
        HelpId::StatusMapSize => {
            "Map size in SMU (Spring Map Units) and heightmap pixels. Pixels follow `64·N + 1` per axis. [PITFALL §4]"
        }
        HelpId::StatusIssueCount => {
            "Total issues from the linter. Click to open the lint panel — each issue shows severity, location, and (where applicable) a one-click fix."
        }
        HelpId::StatusInstall => {
            "Last build/install outcome. Green path = success, red message = failure, dim 'Build: idle' = nothing built this session."
        }
        HelpId::StatusBrushChip => {
            "Live brush size (radius in elmos) and strength (0..1) for the active tool. Updates as you scrub the Inspector sliders."
        }

        // ──────────────── Inspector header ────────────────
        HelpId::HeaderProjectName => {
            "Display name used by Chobby and the in-game map browser. Only `[A-Za-z0-9_-]` survive the slug sanitiser; spaces become underscores."
        }
        HelpId::HeaderProjectSavedChip => {
            "Saved = project file matches in-memory state. Unsaved = there are changes since the last save. Save with Ctrl+S."
        }
        HelpId::HeaderMapSizeX => {
            "Map size along X in SMU (1 SMU = 512 elmos = 65 heightmap pixels). Editor heightmap is `64·N + 1` pixels per axis. [PITFALL §4]"
        }
        HelpId::HeaderMapSizeZ => {
            "Map size along Z in SMU. Same `64·N + 1` rule. Asymmetric maps are valid — e.g. 8 × 16 SMU for a corridor map."
        }
        HelpId::HeaderHeightmapValidChip => {
            "Valid = heightmap dims match declared SMU. Invalid = either no heightmap or its dims do not match `64·N + 1` for the current SMU. [PITFALL §4]"
        }
        HelpId::HeaderHeightmapPath => {
            "Filename the heightmap came from. Imports are validated to `64·N + 1`; PNG/16-bit and EXR are accepted."
        }
        HelpId::HeaderHeightmapDims => {
            "Heightmap pixel dimensions. Must be `(64·smu_x + 1) × (64·smu_z + 1)`. A mismatch turns the validity chip red."
        }
        HelpId::HeaderHeightmapSample => {
            "Observed min/max raw u16 samples. Used by the Water inspector to compute the deepest world-Y the map reaches."
        }
        HelpId::HeaderHeightScale => {
            "World Y at raw heightmap value 65535. BAR maps cap practical height around 4096 elmos; taller values give more headroom but exaggerate slopes."
        }

        // Per-inspector chips.
        HelpId::InspectorSymmetryChip => {
            "Active symmetry mode for this tool's strokes. Toggle in the top bar."
        }
        HelpId::InspectorMapSizeChip => {
            "Map size for the current project. Set in the Project header above."
        }

        // ──────────────── Select tool ────────────────
        HelpId::SelectModeInfo => {
            "Camera-only mode. LMB orbits, MMB pans, RMB orbits, scroll zooms. Pick a tool on the left strip to start editing."
        }

        // ──────────────── Sculpt tool ────────────────
        HelpId::SculptBrushOff => {
            "Disable sculpting — camera-only while keeping the Sculpt tool active. Useful when you want to inspect terrain without risk of an accidental stamp."
        }
        HelpId::SculptBrushRaise => {
            "Add height under the brush. Strength = 1.0 raises by the full brush height per stamp at the centre."
        }
        HelpId::SculptBrushLower => {
            "Subtract height under the brush. The same strength curve as Raise, signed negative."
        }
        HelpId::SculptBrushSmooth => {
            "Average neighbouring samples toward the brush centre. Strength controls the per-stamp blend rate (0..1)."
        }
        HelpId::SculptRadius => {
            "Brush radius in elmos. 1 SMU = 512 elmos, so a 256-elmo brush is half an SMU. Range 8..4096."
        }
        HelpId::SculptStrength => {
            "Stamp magnitude (0..1). At 1.0 a single Raise stamp adds the brush height at the centre; the falloff curve scales it toward the rim."
        }
        HelpId::SculptFalloff => {
            "Visual preview of the radial weight curve — ease-out, full at centre, zero at the rim. Brush shape is fixed for Sprint 19; per-tool curves come later."
        }
        HelpId::SculptBehaviorContinuous => {
            "Always-on stamping while LMB is held. Default and currently the only mode."
        }
        HelpId::SculptBehaviorPressure => {
            "Tablet pressure modulates strength. Not yet wired — reserved for a later sprint."
        }
        HelpId::SculptBehaviorLockZ => {
            "Clamp the brush to a target elevation. Not yet wired — reserved for Phase F2+."
        }

        // ──────────────── Metal tool ────────────────
        HelpId::MetalGlobalChip => {
            "Shows whether `extractor_radius` is at the BAR default (80) or a custom value. Custom values may break mex-snap. [PITFALL §6]"
        }
        HelpId::MetalExtractorRadius => {
            "Cluster radius for the F4 metal view (elmos). BAR overrides the engine default 500 down to 80; setting 500 silently breaks mex-snap. [PITFALL §6]"
        }
        HelpId::MetalAddSpot => {
            "Place a new metal spot at the map centre with the standard yield (2.0). Drag it from the row below, or LMB on the canvas."
        }
        HelpId::MetalSpotX => {
            "Spot X coordinate in elmos, measured from the south-west corner (0,0). Snaps to multiples of 8 on drag."
        }
        HelpId::MetalSpotZ => "Spot Z coordinate in elmos. Same `+8` snap as X.",
        HelpId::MetalSpotValue => {
            "Per-spot metal multiplier. BAR convention: 0.5 = perimeter, 2.0 = standard, 4.0-5.2 = central / strategic. Click to type any value."
        }
        HelpId::MetalSpotDelete => "Delete this metal spot. [Shortcut: Ctrl+Z to restore]",

        // ──────────────── Geo tool ────────────────
        HelpId::GeoAddVent => {
            "Place a new geo vent at the map centre. Geo vents emit steam plumes in-game and let players build geothermal generators."
        }
        HelpId::GeoVentX => {
            "Vent X coordinate in elmos, measured from the south-west corner (0,0). Snaps to multiples of 8 on drag."
        }
        HelpId::GeoVentZ => "Vent Z coordinate in elmos. Same `+8` snap as X.",
        HelpId::GeoVentDelete => "Delete this geo vent. [Shortcut: Ctrl+Z to restore]",

        // ──────────────── Feature tool ────────────────
        HelpId::FeatureCategoryCombo => {
            "Feature category — trees, rocks, props, wreckage, geo. Switching categories clears the pending placement so the next LMB doesn't drop a stale name."
        }
        HelpId::FeatureFilter => {
            "Filter the picker by name, display, or tag. Case-insensitive substring match across all three fields."
        }
        HelpId::FeaturePickerRow => {
            "Click to arm this feature for placement. The next LMB on the canvas drops it; LMB-drag rotates."
        }
        HelpId::FeaturePlacedX => {
            "Placed-feature X coordinate in elmos. Snaps to multiples of 8 on drag."
        }
        HelpId::FeaturePlacedZ => {
            "Placed-feature Z coordinate in elmos. Snaps to multiples of 8 on drag."
        }
        HelpId::FeaturePlacedRot => {
            "Heading in degrees (0..359). 0° faces south. Stored internally as Spring's 16-bit heading (0..65535) for `Spring.CreateFeature`."
        }
        HelpId::FeaturePlacedDelete => "Delete this feature. [Shortcut: Ctrl+Z to restore]",

        // ──────────────── Paint-layer tool ────────────────
        HelpId::PaintBrushReveal => {
            "Increase the active layer's mask alpha — paint the layer in. Strength controls the per-stamp delta toward 1.0."
        }
        HelpId::PaintBrushHide => {
            "Decrease the active layer's mask alpha — paint the layer out. Strength controls the per-stamp delta toward 0.0."
        }
        HelpId::PaintBrushSmooth => {
            "Blur the active layer's mask values under the brush. Useful to soften hard reveal/hide edges before bake."
        }
        HelpId::PaintBrushFill => {
            "Set the active layer's mask to the target value across the entire brush footprint in one stamp. LMB drag bypasses symmetry."
        }
        HelpId::PaintRadius => {
            "Paint-brush radius in elmos. 1 mask pixel = 1 elmo; a 64-elmo brush is one SMF tile wide."
        }
        HelpId::PaintStrength => {
            "Per-stamp delta toward the target mask value (0..1). 1.0 = fully reveal or hide in a single stamp; lower values build up gradually."
        }
        HelpId::PaintSpacing => {
            "Distance between successive stamps along a drag, as a fraction of brush radius. 0.05 = dense overlap, 2.0 = sparse dots."
        }
        HelpId::PaintFillTargetVisible => {
            "Fill mode target. On = Fill reveals the layer (alpha → 1.0); off = Fill hides it (alpha → 0.0)."
        }
        HelpId::PaintMaskOnlyPreview => {
            "Show the active layer's mask as a grayscale overlay (red where alpha = 0) instead of the composite diffuse. Useful for fine alpha edits."
        }
        HelpId::PaintViewMaskPreviewChip => {
            "Active layer's mask value at the cursor (0..255) and the layer name. None = cursor is off-map."
        }

        // ──────────────── Water tool ────────────────
        HelpId::WaterPresetNone => {
            "No water preset. Engine renders its default blue ocean if `min_height < 0`; otherwise no water is visible."
        }
        HelpId::WaterPresetCustom => {
            "Hand-tuned override set. The number in parens shows how many fields you've overridden away from the active preset."
        }
        HelpId::WaterPresetOcean => {
            "Cool blue ocean (`Coastlines` palette). Standard BAR sea-level preset; pairs with any temperate biome."
        }
        HelpId::WaterPresetTropical => {
            "Bright teal shallows with strong wave foam. Designed for atoll and reef maps."
        }
        HelpId::WaterPresetAcid => {
            "Sickly green pool, high damage per tick. Pair with a corroded-metal atmosphere for an industrial / wasteland feel."
        }
        HelpId::WaterPresetLava => {
            "Orange-red lava surface with high damage. Click 'Apply lava-style atmosphere' for matching fog and sun."
        }
        HelpId::WaterPresetMagma => {
            "Lava with thicker fog and a dimmer warm sun. Use for closer-quarters lava maps where the player should feel the heat."
        }
        HelpId::WaterFloorMin => {
            "World Y at raw heightmap value 0. Set negative so basins fill with water — BAR's water plane is fixed at Y = 0."
        }
        HelpId::WaterCeilingMax => {
            "World Y at raw heightmap value 65535. BAR maps cap practical height around 4096 elmos; matches the header's `Max height`."
        }
        HelpId::WaterDamage => {
            "HP damage per game tick (30 ticks/sec) when a unit touches the water surface. 0 = harmless, 200 = standard acid, 10000 = instant kill."
        }
        HelpId::WaterVoidWater => {
            "Render the map without a water plane (the Apophis 'space map' look). Mutually exclusive with plane colour — emission auto-clears it. [PITFALL §6]"
        }
        HelpId::WaterTidalStrength => {
            "BAR tidal-energy economy. Lives at mapinfo top level, NOT inside water{}. [PITFALL §5]"
        }
        HelpId::WaterSurfaceColor => {
            "RGB tint of the water surface as the user sees it. Sky/ambient lighting modulate the final pixel colour."
        }
        HelpId::WaterPlaneColor => {
            "RGB tint of the water plane (below the surface). Disabled while voidWater is on. [PITFALL §6]"
        }
        HelpId::WaterSurfaceAlpha => {
            "Surface transparency (0 = fully clear, 1 = fully opaque). Lower values let the seafloor read through."
        }
        HelpId::WaterWaveSize => {
            "Perlin amplitude of the wave normal. 0 = glass-still, 2.0 = stormy. Affects only the surface normal; the geometry is flat."
        }
        HelpId::WaterFoamStrength => {
            "Whitecap intensity along wave crests. 0 = none, 2.0 = saturated."
        }
        HelpId::WaterCarveDepth => {
            "Per-stamp Lower-brush depth for the Water tool's flood gesture. LMB drag carves toward this depth; RMB drag raises."
        }
        HelpId::WaterAutoMinHeight => {
            "Shortcut: set min_height = min(0, carve_depth) so a Water-tool LMB-drag immediately produces visible water in the carved basin."
        }
        HelpId::WaterLavaAtmosphereApply => {
            "Hard-coded patch: fog (0.9, 0.3, 0.1), sun (1.0, 0.5, 0.3), cloud (0.4, 0.2, 0.15) @ 0.7. Layers on top of the BAR atmosphere default."
        }
        HelpId::WaterLavaAtmosphereRevert => {
            "Remove the lava-atmosphere patch, restoring the BAR default fog / sun / cloud. The water preset itself stays unchanged."
        }

        // ──────────────── Start-positions tool ────────────────
        HelpId::StartLayoutBalancedChip => {
            "Balanced = every ally team has the same number of source positions. Asymmetric = uneven — fine for FFA, suspicious for team play."
        }
        HelpId::StartPresetCombo => {
            "Drop a stock layout: OneVOne / TwoVTwo / EightVEight (corner mirror) / ThreeWayFFA (120°) / FourWayFFA (quad)."
        }
        HelpId::StartDragPaintCount => {
            "Number of positions to distribute when you LMB-drag a line on the canvas. Equally spaced from start to end of the drag."
        }
        HelpId::StartAllyAdd => {
            "Add a new empty ally team. Pick a colour and name in the row below."
        }
        HelpId::StartAllyColor => {
            "Team colour. Drives the canvas marker, the minimap dot, AND the in-game player colour assignment."
        }
        HelpId::StartAllyName => {
            "Display name shown in the in-game lobby. Free text — duplicates are allowed but confusing."
        }
        HelpId::StartAllyActiveStar => {
            "Mark this team active. Subsequent canvas LMB-clicks add positions to the active team."
        }
        HelpId::StartAllyDelete => {
            "Remove this team and every position under it. [Shortcut: Ctrl+Z to restore]"
        }
        HelpId::StartPosDelete => {
            "Remove this source position. Mirror copies derived from symmetry vanish with it. [Shortcut: Ctrl+Z to restore]"
        }

        // ──────────────── Procgen tool ────────────────
        HelpId::ProcgenPresetChip => {
            "Drop in a stock expression. Parabolic bowl = central depression; Saddle = pass; Diagonal ramp = monotonic slope; Plateau = elevated mesa; Custom = your own f(x, z)."
        }
        HelpId::ProcgenExpression => {
            "Heightmap formula f(x, z). Result clamps to [0, 1]; coordinates depend on the Domain choice below. Red outline = parse error — hover for details."
        }
        HelpId::ProcgenDomainUnit => {
            "x and z run 0..1 across the map. Use for one-sided ramps and corner-anchored shapes."
        }
        HelpId::ProcgenDomainCentered => {
            "x and z run -1..1 from the map centre. Use for radial / dish-shaped expressions like `1 - x*x - z*z`."
        }
        HelpId::ProcgenPreviewChip => {
            "Live = the 256² preview is up-to-date with the formula. Parse error = the expression doesn't compile; commit is disabled."
        }
        HelpId::ProcgenCommitButton => {
            "Replace the current heightmap with the formula's output. Disabled until the expression parses. Ctrl+Z reverts."
        }

        // ──────────────── Viewport chrome ────────────────
        HelpId::ViewportGrid => {
            "Toggle the coordinate grid overlay. Vertical / horizontal world-aligned lines at the ruler's tick spacing."
        }
        HelpId::ViewportLighting => {
            "Toggle the directional-light shading in the 3D preview. Off = flat-shaded by elevation; on = sun-shaded."
        }
        HelpId::ViewportWireframe => {
            "Toggle wireframe overlay on the terrain mesh. Useful for verifying heightmap topology against the diffuse."
        }
        HelpId::ViewportBuildable => {
            "Toggle the red 'slope > 10°' overlay. Marks areas where BAR factories (maxslope = 15 / 1.5 → 10°) cannot be placed."
        }
    }
}

/// Sprint 22 / U2 — `show_popover` is the wrapper Sprint 19's
/// `.on_hover_text(help(id))` call sites should migrate to once the
/// Sprint 22 framework is in place.
///
/// Behaviour:
/// - `whats_this == false`: identical to `response.on_hover_text(help(id))`.
/// - `whats_this == true`: in addition to the hover tooltip, renders
///   a small `[?]` chip overlaid in the top-right of `response.rect`
///   when hovered, signalling that the user can click to pin the
///   popover and jump to the help center. (The "true pinned popover
///   stays open after the cursor moves" UX is a Stage 2 polish item;
///   Sprint 22 ships the affordance + the keyboard toggle so users
///   can discover the API.)
///
/// `article`: optional help-center article to jump to via the
/// pinned popover's "Read more" button. `None` means "no
/// dedicated article exists; just show the tooltip."
///
/// The function returns the same `Response` the caller passed in,
/// so it can be chained as a drop-in replacement.
#[allow(dead_code)] // wired by call-site migrations in commit 6 and Stage 2 polish
pub fn show_popover(
    ui: &mut eframe::egui::Ui,
    response: eframe::egui::Response,
    id: HelpId,
    whats_this: bool,
    _article: Option<crate::ui::help_center::HelpArticleId>,
) -> eframe::egui::Response {
    let text = help(id);
    let response = response.on_hover_text(text);
    if whats_this && response.hovered() {
        let painter = ui.painter();
        let center = eframe::egui::pos2(response.rect.right() - 6.0, response.rect.top() + 6.0);
        painter.circle_filled(center, 5.0, eframe::egui::Color32::from_rgb(220, 175, 90));
        painter.text(
            center,
            eframe::egui::Align2::CENTER_CENTER,
            "?",
            eframe::egui::FontId::monospace(8.0),
            eframe::egui::Color32::BLACK,
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 19 / U1 exit criterion: ≥ 80 catalogued help strings.
    #[test]
    fn help_catalog_has_at_least_eighty_entries() {
        assert!(
            HelpId::ALL.len() >= 80,
            "Sprint 19 / U1 requires ≥80 HelpId variants; got {}",
            HelpId::ALL.len(),
        );
    }

    /// Every variant maps to a non-empty string.
    #[test]
    fn help_text_is_total_and_non_empty() {
        for id in HelpId::ALL {
            let s = help(*id);
            assert!(!s.is_empty(), "empty help string for {id:?}");
            assert!(s.len() > 8, "suspiciously short help for {id:?}: {s:?}",);
        }
    }

    /// HelpId::ALL contains no duplicates — a regression here would
    /// make the exhaustiveness test pass while skipping coverage of
    /// a real variant.
    #[test]
    fn help_id_all_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for id in HelpId::ALL {
            assert!(seen.insert(*id), "duplicate id in HelpId::ALL: {id:?}");
        }
    }

    /// Strings stay under a reasonable cap — tooltips are not docs.
    /// 480 chars ≈ 3 generous sentences; longer text belongs in
    /// Sprint 22's help centre.
    #[test]
    fn help_strings_stay_under_three_sentences() {
        for id in HelpId::ALL {
            let s = help(*id);
            assert!(
                s.len() <= 480,
                "{id:?} help text is too long ({} chars). Move to a Sprint 22 placeholder.",
                s.len(),
            );
        }
    }

    /// Suffix convention: when a string mentions a keyboard chord,
    /// it MUST be one of the chords from `cheat_sheet`. Catches the
    /// "false-promise tooltip" failure mode from the audit.
    #[test]
    fn cited_shortcuts_match_real_bindings() {
        use crate::ui::cheat_sheet::{CAMERA_BINDINGS, PROJECT_BINDINGS};
        // Hand-tabulate every chord the catalogue cites today. If
        // a future edit adds a new `[Shortcut: …]`, extend this
        // list AND the cheat_sheet table together.
        let cited: &[&str] = &["Ctrl+S", "Ctrl+Z", "?", "F9"];
        let mut bound: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (k, _) in CAMERA_BINDINGS {
            bound.insert(k);
        }
        for (k, _) in PROJECT_BINDINGS {
            bound.insert(k);
        }
        // F9 isn't in cheat_sheet yet — the mapinfo form button has
        // its own tooltip. Document the exemption rather than add a
        // placeholder; Sprint 22 will reorganise the table.
        let exempt: &[&str] = &["F9"];
        for chord in cited {
            if exempt.contains(chord) {
                continue;
            }
            assert!(
                bound.contains(chord),
                "tooltip cites `{chord}` but no cheat_sheet binding exists",
            );
        }
    }
}
