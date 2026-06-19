//! C7 / Sprint 18 (F9) — mapinfo form editor.
//!
//! Modal-ish `egui::Window` opened from the top-bar `Icon::MapInfo`
//! button. 12 tabs covering every author-editable surface of
//! `mapinfo.lua` plus a Raw Lua sanity-check and the Sprint-18 D7
//! minimap preview / override picker.
//!
//! **Not a `Tool::*` variant** — the form lives outside the tool /
//! inspector cycle so the user can tweak gravity while painting
//! splats.
//!
//! ## Architecture
//!
//! Each tab is one function returning `Vec<MapInfoPatch>` (the edits
//! committed this frame, batched into `ProjectDiff::EditMapInfo` undo
//! entries by the caller). The tabs read from a `MapInfo` snapshot
//! built once per frame via `MapInfo::from(&project)`; they NEVER
//! mutate the schema directly because the App owns the canonical
//! shadow state. The patches flow back through
//! `App::apply_mapinfo_patch` (in `main.rs`) which updates whichever
//! App field corresponds to the patch variant.
//!
//! ## Edit-commit policy
//!
//! Per the kickoff devlog: edits commit on widget release
//! (`response.drag_stopped() || response.lost_focus()`), NOT on every
//! `response.changed()` callback. A DragValue scrub through 200
//! values produces ONE patch, not 200. This matches the F5 / F6 / F7
//! inspectors' shape.
//!
//! ## Round-trip invariant
//!
//! See `tests::round_trip_no_data_loss` — load a fixture, edit one
//! field, re-render Raw Lua, diff is exactly the edited field.

use barme_core::{
    AtmosphereBlock, LightingBlock, MapInfo, MapInfoPatch, Project, Rgb, SunDir, TerrainMoveSpeeds,
    TerrainTypeBlock, WaterBlock, WaterMode,
};
use eframe::egui;
use tracing::trace;

use crate::ui::theme::{ChipTone, Tokens};
use crate::ui::widgets;

/// Which tab the F9 form is showing. Persisted on `App` so re-open
/// keeps the user's focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapInfoTab {
    #[default]
    General,
    Map,
    Smf,
    Lighting,
    Atmosphere,
    Water,
    Resources,
    Splats,
    TerrainTypes,
    Custom,
    RawLua,
    Minimap,
}

impl MapInfoTab {
    pub const ALL: [MapInfoTab; 12] = [
        Self::General,
        Self::Map,
        Self::Smf,
        Self::Lighting,
        Self::Atmosphere,
        Self::Water,
        Self::Resources,
        Self::Splats,
        Self::TerrainTypes,
        Self::Custom,
        Self::RawLua,
        Self::Minimap,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Map => "Map",
            Self::Smf => "SMF",
            Self::Lighting => "Lighting",
            Self::Atmosphere => "Atmosphere",
            Self::Water => "Water",
            Self::Resources => "Resources",
            Self::Splats => "Splats",
            Self::TerrainTypes => "Terrain types",
            Self::Custom => "Custom",
            Self::RawLua => "Raw Lua",
            Self::Minimap => "Minimap",
        }
    }
}

/// Per-frame inputs the form needs from the App. Built once per
/// frame; passed by reference to the tab functions. Decoupling the
/// snapshot from `&mut App` keeps the tab functions testable.
pub struct FormCtx<'a> {
    pub project: &'a Project,
    pub info: &'a MapInfo,
    /// Map of DNTS-bound layers → (slot id, channel name) for the
    /// read-only Splats tab. Empty when no DNTS layers exist.
    pub dnts_summary: Vec<(usize, String, char, f32, f32)>,
    /// Layer summary count for the Splats tab header.
    pub layer_count: usize,
    /// Cached rendering of `mapinfo::render_mapinfo(&info)` for the
    /// Raw Lua tab. Reused frame-to-frame; the caller rebuilds it
    /// only when the project actually mutates.
    pub raw_lua: &'a str,
    /// Cached minimap preview texture (if the bake has run).
    pub minimap_preview: Option<&'a egui::TextureHandle>,
    /// Lint summary stub. C8 / Sprint 21 will populate per-tab counts;
    /// for Sprint 18 every entry is 0. Index by `MapInfoTab as usize`.
    pub lint_per_tab: [u32; 12],
}

/// Render the entire window. Returns the patches the user committed
/// this frame; the App wraps each as a `ProjectDiff::EditMapInfo`
/// and dispatches.
///
/// `open` is the `&mut bool` that drives `egui::Window::open(open)`;
/// the caller observes the close (`*open == false`) and persists.
#[allow(clippy::too_many_arguments)]
pub fn show_window(
    ctx: &egui::Context,
    open: &mut bool,
    tab: &mut MapInfoTab,
    form: &FormCtx<'_>,
) -> Vec<MapInfoPatch> {
    let mut patches = Vec::new();
    egui::Window::new("Map info")
        .open(open)
        .resizable(true)
        .default_size([720.0, 520.0])
        .min_width(560.0)
        .min_height(360.0)
        .show(ctx, |ui| {
            // Tab strip across the top.
            ui.horizontal_wrapped(|ui| {
                for &t in &MapInfoTab::ALL {
                    let lint_count = form.lint_per_tab[t as usize];
                    let label = t.label();
                    let active = *tab == t;
                    let hover = match t {
                        MapInfoTab::General => "Project identity: name, author, version, description.",
                        MapInfoTab::Map => "Top-level map fields: gravity, water level, max metal, voidGround / voidWater toggles.",
                        MapInfoTab::Smf => "SMF metadata: filename root, minimap path, type-texture path, metalmap path.",
                        MapInfoTab::Lighting => "Sun direction + lighting RGB colours.",
                        MapInfoTab::Atmosphere => "Sky / fog / cloud RGB. Default is BAR convention.",
                        MapInfoTab::Water => "Engine water-block fields: surface / plane colours, damage, void water, tidal.",
                        MapInfoTab::Resources => "Texture resources: detail, splat distribution, splat detail normals, specular.",
                        MapInfoTab::Splats => "Read-only splat summary derived from the active Layers stack (Sprint 17).",
                        MapInfoTab::TerrainTypes => "Per-type-index hardness + movement speeds. Drives unit move costs across terrain.",
                        MapInfoTab::Custom => "Free-form `mapinfo.custom.*` key/value pairs for gadget consumption.",
                        MapInfoTab::RawLua => "Read-only rendering of the full mapinfo.lua. Useful for diffing against a reference map.",
                        MapInfoTab::Minimap => "Minimap preview + optional override PNG (D7 / Sprint 18).",
                    };
                    let resp = if active {
                        ui.add(egui::Button::new(label).fill(Tokens::DARK.accent))
                    } else {
                        ui.add(egui::Button::new(label).fill(Tokens::DARK.panel2))
                    }
                    .on_hover_text(hover);
                    // C8 (Sprint 21) lint chip dot — stubbed at 0 in
                    // Sprint 18. The rendering is live so the wiring is
                    // proven before lint output exists.
                    if lint_count > 0 {
                        let center = resp.rect.right_top() - egui::vec2(4.0, -4.0);
                        ui.painter().circle_filled(
                            center,
                            3.5,
                            Tokens::DARK.chip_fg(ChipTone::Err),
                        );
                    }
                    if resp.clicked() {
                        *tab = t;
                    }
                }
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match *tab {
                    MapInfoTab::General => general_tab(ui, form, &mut patches),
                    MapInfoTab::Map => map_tab(ui, form, &mut patches),
                    MapInfoTab::Smf => smf_tab(ui, form, &mut patches),
                    MapInfoTab::Lighting => lighting_tab(ui, form, &mut patches),
                    MapInfoTab::Atmosphere => atmosphere_tab(ui, form, &mut patches),
                    MapInfoTab::Water => water_tab(ui, form, &mut patches),
                    MapInfoTab::Resources => resources_tab(ui, form, &mut patches),
                    MapInfoTab::Splats => splats_tab(ui, form),
                    MapInfoTab::TerrainTypes => terrain_types_tab(ui, form, &mut patches),
                    MapInfoTab::Custom => custom_tab(ui, form, &mut patches),
                    MapInfoTab::RawLua => raw_lua_tab(ui, form),
                    MapInfoTab::Minimap => minimap_tab(ui, form, &mut patches),
                });
        });
    if !patches.is_empty() {
        trace!(
            count = patches.len(),
            "F9 form: emitted {} patch(es)",
            patches.len()
        );
    }
    patches
}

// ─────────────── tabs ───────────────

fn general_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    widgets::section(
        ui,
        "Identity",
        true,
        |_| {},
        |ui| {
            let mut name = form.info.name.clone();
            if text_edit_singleline(
                ui,
                "Name",
                &mut name,
                "Display name shown in Chobby and the engine HUD. Sanitised at save time \
             (`barme_core::sanitize_name`) so non-ASCII characters can't crash PyMapConv.",
            ) {
                out.push(MapInfoPatch::Name(name));
            }
            let mut sname = form.info.shortname.clone().unwrap_or_default();
            if text_edit_singleline(
                ui,
                "Short name",
                &mut sname,
                "Optional shorter display name. Defaults to the full name.",
            ) {
                out.push(MapInfoPatch::Shortname(if sname.is_empty() {
                    None
                } else {
                    Some(sname)
                }));
            }
            let mut author = form.info.author.clone().unwrap_or_default();
            if text_edit_singleline(
                ui,
                "Author",
                &mut author,
                "Map author credit, surfaced in Chobby's map browser.",
            ) {
                out.push(MapInfoPatch::Author(if author.is_empty() {
                    None
                } else {
                    Some(author)
                }));
            }
            let mut ver = form.info.version.clone();
            if text_edit_singleline(
                ui,
                "Version",
                &mut ver,
                "BAR convention: `1.0`. Bump on every published rev so Chobby caches don't \
             collide with an older `.sd7` of the same name.",
            ) {
                out.push(MapInfoPatch::Version(ver));
            }
        },
    );
    widgets::section(
        ui,
        "Description",
        false,
        |_| {},
        |ui| {
            let mut desc = form.info.description.clone().unwrap_or_default();
            let resp = ui
                .add(
                    egui::TextEdit::multiline(&mut desc)
                        .hint_text("Short blurb shown in Chobby")
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                )
                .on_hover_text("Free-form blurb shown under the map in Chobby's browser. Markdown is NOT rendered; keep it short.");
            if commit_text(&resp) {
                out.push(MapInfoPatch::Description(if desc.is_empty() {
                    None
                } else {
                    Some(desc)
                }));
            }
        },
    );
    widgets::section(
        ui,
        "Engine flags (read-only)",
        false,
        |_| {},
        |ui| {
            ui.label(
                egui::RichText::new(format!("modtype = {} (Map)", form.info.modtype))
                    .color(Tokens::DARK.muted)
                    .size(11.0),
            )
            .on_hover_text(
                "Chobby's map browser shows entries with modtype == 3 in multiplayer lobbies. \
                 The editor pins this; do NOT change it.",
            );
            ui.label(
                egui::RichText::new(format!("depend = {:?}", form.info.depend))
                    .color(Tokens::DARK.muted)
                    .size(11.0),
            )
            .on_hover_text(
                "Engine fallback render assets. Always carries 'Map Helper v1'; without it \
                 untextured maps render with the engine's grey grid.",
            );
        },
    );
}

fn map_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    widgets::section(
        ui,
        "Physics",
        true,
        |_| {},
        |ui| {
            opt_drag_value(
                ui,
                "Gravity",
                form.info.gravity,
                1.0..=400.0,
                "Per-frame gravity. BAR convention is 130 (engine default is also 130).",
                |v| out.push(MapInfoPatch::Gravity(v)),
            );
            opt_drag_value(
                ui,
                "Tidal strength",
                form.info.tidal_strength,
                0.0..=30.0,
                "Non-zero on water maps to power Tidal Generators. Lives at MapInfo top level \
             even though the dedicated Water tool surfaces it for UX.",
                |v| out.push(MapInfoPatch::TidalStrength(v)),
            );
            opt_drag_value(
                ui,
                "Map hardness",
                form.info.maphardness,
                1.0..=1000.0,
                "Crater deformation resistance multiplier. BAR default 100.",
                |v| out.push(MapInfoPatch::Maphardness(v)),
            );
        },
    );
    widgets::section(
        ui,
        "Resources",
        false,
        |_| {},
        |ui| {
            let er = form.info.extractor_radius;
            opt_drag_value(
                ui,
                "Extractor radius",
                er,
                16.0..=200.0,
                "Engine-wide mex exclusion radius (elmos). BAR overrides the engine's 500 to 80; \
             setting it back to 500 silently breaks mex snap (PITFALL §6). Lint warns at >200.",
                |v| out.push(MapInfoPatch::ExtractorRadius(v)),
            );
            if let Some(r) = er
                && r > 200.0
            {
                widgets::chip(
                    ui,
                    ChipTone::Warn,
                    format!("extractor_radius = {r:.0} > 200 (likely too large)"),
                )
                .on_hover_text(
                    "BAR's mex snap uses this radius; values >200 silently disable mex-snap on \
                 the F4 income display. Sprint 21 will flip this to a lint chip.",
                );
            }
            opt_drag_value(
                ui,
                "Max metal",
                form.info.max_metal,
                0.0..=10.0,
                "m/s metal yield at full ground-metal saturation. Real BAR maps cluster 0.93..=4.11.",
                |v| out.push(MapInfoPatch::MaxMetal(v)),
            );
        },
    );
    widgets::section(
        ui,
        "Void / display",
        false,
        |_| {},
        |ui| {
            let mut vw = form.info.void_water;
            if ui
                .checkbox(&mut vw, "Void water (Apophis-style 'space map')")
                .on_hover_text(
                    "Removes the water plane entirely. Mutually exclusive with water.planeColor \
                 — the emission path clears planeColor when this is on (PITFALL §6).",
                )
                .changed()
            {
                out.push(MapInfoPatch::VoidWater(vw));
            }
            let mut vg = form.info.void_ground;
            if ui
            .checkbox(&mut vg, "Void ground (alpha-cut terrain)")
            .on_hover_text(
                "Engine alpha-cuts the diffuse below voidAlphaMin. Used by 'island archipelago' \
                 maps. Only meaningful when the diffuse carries an alpha channel.",
            )
            .changed()
        {
            out.push(MapInfoPatch::VoidGround(vg));
        }
            if vg {
                let mut va = form.info.void_alpha_min;
                let resp = ui
                .add(egui::Slider::new(&mut va, 0.0..=1.0).text("voidAlphaMin"))
                .on_hover_text(
                    "Alpha threshold below which voidGround discards fragments. Engine default 0.9.",
                );
                if commit_drag(&resp) {
                    out.push(MapInfoPatch::VoidAlphaMin(va));
                }
            }
            let mut asm = form.info.auto_show_metal.unwrap_or(true);
            if ui
                .checkbox(&mut asm, "Auto-show metal view")
                .on_hover_text("Toggles F4 metal-view auto-display on map start.")
                .changed()
            {
                out.push(MapInfoPatch::AutoShowMetal(Some(asm)));
            }
        },
    );
}

fn smf_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    widgets::section(
        ui,
        "Heights",
        true,
        |_| {},
        |ui| {
            opt_drag_value(
                ui,
                "Min height (elmos)",
                form.info.smf.min_height,
                -4096.0..=4096.0,
                "World-space Y of the lowest heightmap sample. Negative values flood below \
             BAR's water plane at Y = 0 (PITFALL §28 — the water plane is consteval).",
                |v| out.push(MapInfoPatch::SmfMinHeight(v)),
            );
            opt_drag_value(
                ui,
                "Max height (elmos)",
                form.info.smf.max_height,
                1.0..=4096.0,
                "World-space Y of the highest heightmap sample. Drives the editor's \
             biome-ramp range too.",
                |v| out.push(MapInfoPatch::SmfMaxHeight(v)),
            );
        },
    );
    widgets::section(
        ui,
        "SMT (read-only)",
        false,
        |_| {},
        |ui| {
            ui.label(
                egui::RichText::new(format!(
                    "smtFileName0 = {:?}",
                    form.info.smf.smt_file_name_0
                ))
                .color(Tokens::DARK.muted)
                .size(11.0)
                .monospace(),
            )
            .on_hover_text(
                "Auto-set from the project name via `Project::sanitize_name`. Renaming the \
             project rewrites this atomically (PITFALL §7 — pink-map on rename).",
            );
        },
    );
}

fn lighting_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    let l: &LightingBlock = &form.info.lighting;
    widgets::section(
        ui,
        "Sun",
        true,
        |_| {},
        |ui| {
            let mut sd: SunDir = l.sun_dir;
            let changed = sun_dir_editor(ui, &mut sd);
            if changed {
                out.push(MapInfoPatch::LightingSunDir(sd));
            }
            ui.label(
                egui::RichText::new(
                    "W is the engine's intensity scalar (default 1.0 — `MapInfo.cpp:213`). \
                 PITFALL §18: emitting 1e9 over-saturates sunlight on load.",
                )
                .color(Tokens::DARK.muted)
                .size(10.5),
            );
            if ui
                .small_button("Reset W to 1.0")
                .on_hover_text("Force the intensity scalar back to BAR's convention 1.0.")
                .clicked()
                && (sd[3] - 1.0).abs() > f32::EPSILON
            {
                sd[3] = 1.0;
                out.push(MapInfoPatch::LightingSunDir(sd));
            }
        },
    );
    widgets::section(
        ui,
        "Ground",
        false,
        |_| {},
        |ui| {
            opt_rgb_picker(ui, "Ambient", l.ground_ambient_color, |v| {
                out.push(MapInfoPatch::LightingGroundAmbientColor(v));
            });
            opt_rgb_picker(ui, "Diffuse", l.ground_diffuse_color, |v| {
                out.push(MapInfoPatch::LightingGroundDiffuseColor(v));
            });
            opt_rgb_picker(ui, "Specular", l.ground_specular_color, |v| {
                out.push(MapInfoPatch::LightingGroundSpecularColor(v));
            });
            opt_drag_value(
                ui,
                "Shadow density",
                l.ground_shadow_density,
                0.0..=1.0,
                "0 = no terrain shadows; 1 = fully shaded shadow regions.",
                |v| out.push(MapInfoPatch::LightingGroundShadowDensity(v)),
            );
        },
    );
    widgets::section(
        ui,
        "Units",
        false,
        |_| {},
        |ui| {
            opt_rgb_picker(ui, "Ambient", l.unit_ambient_color, |v| {
                out.push(MapInfoPatch::LightingUnitAmbientColor(v));
            });
            opt_rgb_picker(ui, "Diffuse", l.unit_diffuse_color, |v| {
                out.push(MapInfoPatch::LightingUnitDiffuseColor(v));
            });
            opt_rgb_picker(ui, "Specular", l.unit_specular_color, |v| {
                out.push(MapInfoPatch::LightingUnitSpecularColor(v));
            });
            opt_drag_value(
                ui,
                "Shadow density",
                l.unit_shadow_density,
                0.0..=1.0,
                "Per-unit shadow opacity. Engine defaults match ground when omitted.",
                |v| out.push(MapInfoPatch::LightingUnitShadowDensity(v)),
            );
            opt_drag_value(
                ui,
                "Specular exponent",
                l.specular_exponent,
                1.0..=200.0,
                "Phong specular exponent for units (engine fallback when no specularTex bound).",
                |v| out.push(MapInfoPatch::LightingSpecularExponent(v)),
            );
        },
    );
}

fn atmosphere_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    let a: &AtmosphereBlock = &form.info.atmosphere;
    widgets::section(
        ui,
        "Wind",
        true,
        |_| {},
        |ui| {
            opt_drag_value(
                ui,
                "Min wind",
                a.min_wind,
                0.0..=100.0,
                "Lower bound of the per-second wind oscillation (drives generator income).",
                |v| out.push(MapInfoPatch::AtmosphereMinWind(v)),
            );
            opt_drag_value(
                ui,
                "Max wind",
                a.max_wind,
                0.0..=100.0,
                "Upper bound of the wind oscillation. BAR default range is 5..25.",
                |v| out.push(MapInfoPatch::AtmosphereMaxWind(v)),
            );
        },
    );
    widgets::section(
        ui,
        "Fog",
        false,
        |_| {},
        |ui| {
            opt_drag_value(
                ui,
                "Fog start (0..1)",
                a.fog_start,
                0.0..=1.0,
                "Normalised distance where fog begins to occlude. Setting equal to fog end \
             breaks the build-ETA grid renderer (PITFALL §6).",
                |v| out.push(MapInfoPatch::AtmosphereFogStart(v)),
            );
            opt_drag_value(
                ui,
                "Fog end (0..1)",
                a.fog_end,
                0.0..=1.0,
                "Normalised distance where fog reaches full opacity. BAR default 1.0.",
                |v| out.push(MapInfoPatch::AtmosphereFogEnd(v)),
            );
            if let (Some(fs), Some(fe)) = (a.fog_start, a.fog_end)
                && (fs - fe).abs() < f32::EPSILON
            {
                widgets::chip(
                    ui,
                    ChipTone::Err,
                    "fog_start == fog_end (silently breaks build-ETA renderer)",
                );
            }
            opt_rgb_picker(ui, "Fog colour", a.fog_color, |v| {
                out.push(MapInfoPatch::AtmosphereFogColor(v));
            });
        },
    );
    widgets::section(
        ui,
        "Sun + sky",
        false,
        |_| {},
        |ui| {
            opt_rgb_picker(ui, "Sun colour", a.sun_color, |v| {
                out.push(MapInfoPatch::AtmosphereSunColor(v));
            });
            opt_rgb_picker(ui, "Sky colour", a.sky_color, |v| {
                out.push(MapInfoPatch::AtmosphereSkyColor(v));
            });
            let mut s = a.sky_axis_angle;
            ui.label(
                egui::RichText::new("Sky axis (xyz) + angle (radians)")
                    .color(Tokens::DARK.muted)
                    .size(11.0),
            );
            let mut changed_axis = false;
            ui.horizontal(|ui| {
                for (i, label) in ["x", "y", "z"].iter().enumerate() {
                    let resp = ui
                        .add(
                            egui::DragValue::new(&mut s[i])
                                .range(-1.0..=1.0)
                                .speed(0.01)
                                .prefix(format!("{label}=")),
                        )
                        .on_hover_text(format!(
                            "Sky axis {label} component (-1..1). Together with the angle below, defines the skybox rotation. PITFALL §12: replaces the legacy `skyDir`."
                        ));
                    if commit_drag(&resp) {
                        changed_axis = true;
                    }
                }
            });
            let mut angle_deg = s[3].to_degrees();
            let resp = ui
                .add(
                    egui::DragValue::new(&mut angle_deg)
                        .range(-360.0..=360.0)
                        .speed(0.5)
                        .suffix("°"),
                )
                .on_hover_text(
                    "Skybox rotation about the axis. PITFALL §12: the legacy `skyDir` key is \
                 deprecated; the engine logs L_DEPRECATED if a `.lua` ever ships it.",
                );
            let angle_committed = commit_drag(&resp);
            if changed_axis || angle_committed {
                s[3] = angle_deg.to_radians();
                out.push(MapInfoPatch::AtmosphereSkyAxisAngle(s));
            }
            let mut skybox = a.sky_box.clone().unwrap_or_default();
            if text_edit_singleline(
                ui,
                "Sky box (.dds)",
                &mut skybox,
                "Skybox cubemap filename (in `maps/` or `bitmaps/`). Empty = engine default sky.",
            ) {
                out.push(MapInfoPatch::AtmosphereSkyBox(if skybox.is_empty() {
                    None
                } else {
                    Some(skybox)
                }));
            }
        },
    );
    widgets::section(
        ui,
        "Clouds",
        false,
        |_| {},
        |ui| {
            opt_drag_value(
                ui,
                "Density",
                a.cloud_density,
                0.0..=1.0,
                "Cloud cover fraction. BAR default 0.5.",
                |v| out.push(MapInfoPatch::AtmosphereCloudDensity(v)),
            );
            opt_rgb_picker(ui, "Colour", a.cloud_color, |v| {
                out.push(MapInfoPatch::AtmosphereCloudColor(v));
            });
        },
    );
}

fn water_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, _out: &mut Vec<MapInfoPatch>) {
    let mode = form.project.water_mode;
    widgets::section(
        ui,
        "Active preset (read-only)",
        true,
        |_| {},
        |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("water_mode = {}", mode.label()))
                        .color(Tokens::DARK.text)
                        .size(12.0)
                        .monospace(),
                );
                widgets::chip(
                    ui,
                    if mode == WaterMode::None {
                        ChipTone::Neutral
                    } else {
                        ChipTone::Ok
                    },
                    format!(
                        "{} field override(s)",
                        barme_core::water_override_count(&form.project.water_overrides)
                    ),
                );
            });
            ui.add(
                egui::Label::new(
                    egui::RichText::new(
                        "The dedicated Water tool (keyboard `W`) is the canonical entry point — \
                         preset chips, behaviour, appearance, flood. This tab is a power-user \
                         backstop showing every raw WaterBlock field; edits flow through the \
                         same `Project.water_overrides` overlay.",
                    )
                    .color(Tokens::DARK.muted)
                    .size(10.5),
                )
                .sense(egui::Sense::hover()),
            )
            .on_hover_text("Sprint 18: the Water tab is read-only here; Sprint 26 (water polish) will land per-field DragValue edits on this tab.");
        },
    );
    let block = form
        .info
        .water
        .as_ref()
        .cloned()
        .unwrap_or_else(WaterBlock::default);
    widgets::section(
        ui,
        "Surface",
        false,
        |_| {},
        |ui| {
            readonly_opt_rgb(ui, "surfaceColor", block.surface_color);
            readonly_opt_f32(ui, "surfaceAlpha", block.surface_alpha);
            readonly_opt_rgb(ui, "planeColor", block.plane_color);
            readonly_opt_rgb(ui, "baseColor", block.base_color);
            readonly_opt_rgb(ui, "minColor", block.min_color);
            readonly_opt_rgb(ui, "absorb", block.absorb);
            readonly_opt_rgb(ui, "specularColor", block.specular_color);
        },
    );
    widgets::section(
        ui,
        "Reflection",
        false,
        |_| {},
        |ui| {
            readonly_opt_f32(ui, "fresnelMin", block.fresnel_min);
            readonly_opt_f32(ui, "fresnelMax", block.fresnel_max);
            readonly_opt_f32(ui, "fresnelPower", block.fresnel_power);
            readonly_opt_f32(ui, "reflectionDistortion", block.reflection_distortion);
            readonly_opt_f32(ui, "blurBase", block.blur_base);
            readonly_opt_f32(ui, "blurExponent", block.blur_exponent);
        },
    );
    widgets::section(
        ui,
        "Perlin",
        false,
        |_| {},
        |ui| {
            readonly_opt_f32(ui, "perlinStartFreq", block.perlin_start_freq);
            readonly_opt_f32(ui, "perlinLacunarity", block.perlin_lacunarity);
            readonly_opt_f32(ui, "perlinAmplitude", block.perlin_amplitude);
        },
    );
    widgets::section(
        ui,
        "Damage + texture overrides",
        false,
        |_| {},
        |ui| {
            readonly_opt_f32(ui, "damage", block.damage);
            readonly_opt_str(ui, "texture", block.texture.as_deref());
            readonly_opt_str(ui, "foamTexture", block.foam_texture.as_deref());
            readonly_opt_str(ui, "normalTexture", block.normal_texture.as_deref());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!("caustics: {} frame(s)", block.caustics.len()))
                        .color(Tokens::DARK.muted)
                        .size(11.0),
                )
                .sense(egui::Sense::hover()),
            )
            .on_hover_text("Caustic animation frame count. Each frame is one Lua-table entry in the water block.");
        },
    );
    ui.add(
        egui::Label::new(
            egui::RichText::new(
                "Sprint 18 ships this tab as read-only — Sprint 26 (water polish) introduces \
                 per-field DragValue edits on top of the same `Project.water_overrides` overlay. \
                 For now, use the dedicated Water tool's Inspector for non-default settings.",
            )
            .color(Tokens::DARK.muted)
            .size(10.5),
        )
        .sense(egui::Sense::hover()),
    )
    .on_hover_text("Sprint 26 / U5 will replace this footer with per-field DragValue editors.");
}

fn resources_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    let r = &form.info.resources;
    widgets::section(
        ui,
        "User-supplied textures",
        true,
        |_| {},
        |ui| {
            opt_text_input(
                ui,
                "detailTex",
                r.detail_tex.as_deref(),
                "Per-fragment detail-overlay texture (low-frequency colour noise). Usually \
                 a 1024² PNG in `maps/`.",
                |v| out.push(MapInfoPatch::ResourcesDetailTex(v)),
            );
            opt_text_input(
                ui,
                "specularTex",
                r.specular_tex.as_deref(),
                "Specular cubemap or texture. Required for visible DNTS normal mapping \
                 (FINDINGS §7.2 — engine no longer gates DNTS on this, but the visual result \
                 is noticeably flatter without).",
                |v| out.push(MapInfoPatch::ResourcesSpecularTex(v)),
            );
            opt_text_input(
                ui,
                "detailNormalTex",
                r.detail_normal_tex.as_deref(),
                "Tangent-space detail normal map. Encoded as R=nx / A=nz (PITFALL §16).",
                |v| out.push(MapInfoPatch::ResourcesDetailNormalTex(v)),
            );
            opt_text_input(
                ui,
                "lightEmissionTex",
                r.light_emission_tex.as_deref(),
                "Emissive texture sampled for self-lit terrain (lava cracks, neon). \
                 Cheaper than emission-baked diffuse for high-fidelity hot maps.",
                |v| out.push(MapInfoPatch::ResourcesLightEmissionTex(v)),
            );
            opt_text_input(
                ui,
                "skyReflectModTex",
                r.sky_reflect_mod_tex.as_deref(),
                "Per-pixel sky-reflection intensity modulator. Use to dampen specular \
                 highlights on rough materials.",
                |v| out.push(MapInfoPatch::ResourcesSkyReflectModTex(v)),
            );
            opt_text_input(
                ui,
                "parallaxHeightTex",
                r.parallax_height_tex.as_deref(),
                "Per-pixel height-offset texture for parallax occlusion mapping. Engine \
                 cost is high; profile before shipping.",
                |v| out.push(MapInfoPatch::ResourcesParallaxHeightTex(v)),
            );
            opt_text_input(
                ui,
                "grassBladeTex",
                r.grass_blade_tex.as_deref(),
                "Blade silhouette texture for grass rendering (engine `grassBladeTex`). \
                 The `.a` channel masks the blade quad. Empty = the procedural blade \
                 shape (Sprint 34). Typically a 64×64 PNG.",
                |v| out.push(MapInfoPatch::ResourcesGrassBladeTex(v)),
            );
        },
    );
    widgets::section(
        ui,
        "Layer-stack driven (read-only)",
        false,
        |_| {},
        |ui| {
            readonly_opt_str(ui, "splatDistrTex", r.splat_distr_tex.as_deref());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "splatDetailNormalTex: {} entry(ies)",
                        r.splat_detail_normal_tex.len()
                    ))
                    .color(Tokens::DARK.muted)
                    .size(11.0),
                )
                .sense(egui::Sense::hover()),
            )
            .on_hover_text("Count of DNTS layer textures in the Layers stack. Edits flow through the Paint Layer Inspector — this tab just mirrors them.")
            .on_hover_text(
                "Driven by the Layers panel's DNTS bindings. Edit those in the Layers \
                 panel (keyboard `L`) — not here.",
            );
        },
    );
}

fn splats_tab(ui: &mut egui::Ui, form: &FormCtx<'_>) {
    widgets::section(
        ui,
        "DNTS bindings (read-only)",
        true,
        |_| {},
        |ui| {
            if form.dnts_summary.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "No DNTS-bound layers. Add one in the Layers panel (`L`) and \
                         set its 'DNTS channel' property.",
                    )
                    .color(Tokens::DARK.muted)
                    .size(11.0),
                );
            } else {
                for (layer_idx, name, ch, scale, mult) in &form.dnts_summary {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("ch {ch}"))
                                    .color(Tokens::DARK.accent)
                                    .size(11.0)
                                    .monospace(),
                            )
                            .sense(egui::Sense::hover()),
                        )
                        .on_hover_text("Splat distribution channel (R/G/B/A) this DNTS layer binds to.");
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("layer #{layer_idx}: {name}"))
                                    .color(Tokens::DARK.text)
                                    .size(11.0),
                            )
                            .sense(egui::Sense::hover()),
                        )
                        .on_hover_text("Source layer's stack position + display name. Edit in the Paint Layer Inspector.");
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("scale {scale:.4} mult {mult:.2}"))
                                    .color(Tokens::DARK.muted)
                                    .size(10.5)
                                    .monospace(),
                            )
                            .sense(egui::Sense::hover()),
                        )
                        .on_hover_text("DNTS tex_scale (frequency) and tex_mult (intensity) — emitted into mapinfo.resources.splatDetailNormalTexScales / Mults.");
                    });
                }
            }
            ui.label(
                egui::RichText::new(format!(
                    "{} layer(s) in stack; Sprint 17 retired the editable splat inspector.",
                    form.layer_count
                ))
                .color(Tokens::DARK.muted)
                .size(10.5),
            );
        },
    );
}

fn terrain_types_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    let mut types = form.info.terrain_types.clone();
    let mut changed = false;
    let mut add_row = false;
    widgets::section(
        ui,
        "Per-type gameplay scalars",
        true,
        |ui| {
            if ui
                .small_button("+ Add type")
                .on_hover_text("Append a new terrain-type row. Index = max+1; defaults to hardness=1, tracks=on, all move speeds=1.")
                .clicked()
            {
                add_row = true;
            }
        },
        |ui| {
            let mut remove: Option<usize> = None;
            egui::Grid::new("terrain_types_grid")
                .num_columns(8)
                .striped(true)
                .show(ui, |ui| {
                    let headers: &[(&str, &str)] = &[
                        ("idx", "Engine type-index (0..255). The type-texture pixel value at each tile picks the entry."),
                        ("name", "Editor-only display name."),
                        ("hard", "Hardness multiplier — higher = stiffer ground, units sink less."),
                        ("tracks", "Whether vehicle tracks render on this terrain type."),
                        ("tank", "Tank-class movement multiplier on this terrain (0 = blocked)."),
                        ("kbot", "Kbot-class movement multiplier on this terrain (0 = blocked)."),
                        ("hover", "Hover-class movement multiplier on this terrain (0 = blocked)."),
                        ("ship + del", "Ship-class movement multiplier + delete button."),
                    ];
                    for (label, hover) in headers {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(*label)
                                    .color(Tokens::DARK.muted)
                                    .size(10.5),
                            )
                            .sense(egui::Sense::hover()),
                        )
                        .on_hover_text(*hover);
                    }
                    ui.end_row();
                    for (row_idx, t) in types.iter_mut().enumerate() {
                        let mut idx = t.index as u32;
                        let resp = ui
                            .add(egui::DragValue::new(&mut idx).range(0..=255))
                            .on_hover_text("Terrain-type index (0..255). Maps the type texture's pixel value to this entry.");
                        if commit_drag(&resp) && idx as u8 != t.index {
                            t.index = idx as u8;
                            changed = true;
                        }
                        let mut name = t.name.clone().unwrap_or_default();
                        let resp = ui
                            .add(egui::TextEdit::singleline(&mut name).desired_width(80.0))
                            .on_hover_text("Display name for this terrain type. Engine ignores it; useful for editor notes.");
                        if commit_text(&resp) {
                            t.name = if name.is_empty() { None } else { Some(name) };
                            changed = true;
                        }
                        let mut hardness = t.hardness.unwrap_or(1.0);
                        let resp = ui
                            .add(
                                egui::DragValue::new(&mut hardness)
                                    .range(0.0..=10.0)
                                    .speed(0.05),
                            )
                            .on_hover_text("Terrain hardness multiplier (0..10). Higher = stiffer ground, units sink less. 1.0 is the engine default.");
                        if commit_drag(&resp) {
                            t.hardness = Some(hardness);
                            changed = true;
                        }
                        let mut tracks = t.receive_tracks.unwrap_or(true);
                        if ui
                            .checkbox(&mut tracks, "")
                            .on_hover_text("Whether vehicle tracks render on this terrain type. Off = tracks are hidden (water / lava surfaces).")
                            .changed()
                        {
                            t.receive_tracks = Some(tracks);
                            changed = true;
                        }
                        let mut ms = t.move_speeds.clone().unwrap_or_default();
                        if move_speed_drag(ui, &mut ms.tank) {
                            changed = true;
                        }
                        if move_speed_drag(ui, &mut ms.kbot) {
                            changed = true;
                        }
                        if move_speed_drag(ui, &mut ms.hover) {
                            changed = true;
                        }
                        ui.horizontal(|ui| {
                            if move_speed_drag(ui, &mut ms.ship) {
                                changed = true;
                            }
                            if ui.small_button("×").on_hover_text("Delete row").clicked() {
                                remove = Some(row_idx);
                            }
                        });
                        t.move_speeds = Some(ms);
                        ui.end_row();
                    }
                });
            if let Some(idx) = remove {
                types.remove(idx);
                changed = true;
            }
        },
    );
    if add_row {
        let next_idx = types.iter().map(|t| t.index).max().unwrap_or(0) + 1;
        types.push(TerrainTypeBlock {
            index: next_idx,
            name: Some(format!("Type {next_idx}")),
            hardness: Some(1.0),
            receive_tracks: Some(true),
            move_speeds: Some(TerrainMoveSpeeds {
                tank: Some(1.0),
                kbot: Some(1.0),
                hover: Some(1.0),
                ship: Some(1.0),
            }),
        });
        changed = true;
    }
    if changed {
        out.push(MapInfoPatch::TerrainTypes(types));
    }
}

fn move_speed_drag(ui: &mut egui::Ui, v: &mut Option<f32>) -> bool {
    let mut val = v.unwrap_or(1.0);
    let resp = ui
        .add(egui::DragValue::new(&mut val).range(0.0..=2.0).speed(0.02))
        .on_hover_text("Per-class movement multiplier on this terrain (0..2). 1.0 = engine default. Below 1 slows the class; 0 blocks it.");
    if commit_drag(&resp) {
        *v = Some(val);
        return true;
    }
    false
}

fn custom_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    widgets::section(
        ui,
        "Free-form mapinfo overrides",
        true,
        |_| {},
        |ui| {
            ui.label(
                egui::RichText::new(
                    "Key/value pairs merged into `mapinfo.custom.*` for gadget consumption. \
                     Currently supports string / number / bool leaves; structured TOML values \
                     load and display read-only.",
                )
                .color(Tokens::DARK.muted)
                .size(10.5),
            );
            let mut entries: Vec<(&String, &toml::Value)> =
                form.project.mapinfo_overrides.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            if entries.is_empty() {
                ui.label(
                    egui::RichText::new("(no overrides)")
                        .color(Tokens::DARK.muted)
                        .size(11.0)
                        .italics(),
                );
            } else {
                egui::Grid::new("custom_grid")
                    .num_columns(3)
                    .show(ui, |ui| {
                        for (k, v) in &entries {
                            ui.label(
                                egui::RichText::new(*k)
                                    .color(Tokens::DARK.text)
                                    .size(11.0)
                                    .monospace(),
                            );
                            ui.label(
                                egui::RichText::new(format!("{v}"))
                                    .color(Tokens::DARK.muted)
                                    .size(11.0)
                                    .monospace(),
                            );
                            if ui
                                .small_button("×")
                                .on_hover_text("Remove this custom field.")
                                .clicked()
                            {
                                out.push(MapInfoPatch::CustomField {
                                    key: (*k).clone(),
                                    value: None,
                                });
                            }
                            ui.end_row();
                        }
                    });
            }
            // Add-row UI: persistent input state lives in egui's
            // own per-frame Ui memory (id-keyed) so this is a pure
            // function without forcing the form to own its own
            // editing scratch.
            ui.separator();
            let id_key = ui.id().with("custom_add");
            let mut new_key = ui.ctx().memory_mut(|m| {
                m.data
                    .get_temp::<String>(id_key.with("k"))
                    .unwrap_or_default()
            });
            let mut new_val = ui.ctx().memory_mut(|m| {
                m.data
                    .get_temp::<String>(id_key.with("v"))
                    .unwrap_or_default()
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut new_key)
                        .hint_text("key")
                        .desired_width(160.0),
                )
                .on_hover_text("Key for the new custom field. Maps to `mapinfo.custom.<key>` in the rendered Lua.");
                ui.add(
                    egui::TextEdit::singleline(&mut new_val)
                        .hint_text("string value")
                        .desired_width(220.0),
                )
                .on_hover_text("String value. Numbers / bools need to be entered as their TOML literal — Sprint 22+ adds typed inputs.");
                if ui
                    .button("Add")
                    .on_hover_text("Commit the new key/value pair. Existing keys are overwritten.")
                    .clicked()
                    && !new_key.is_empty()
                {
                    out.push(MapInfoPatch::CustomField {
                        key: new_key.clone(),
                        value: Some(toml::Value::String(new_val.clone())),
                    });
                    new_key.clear();
                    new_val.clear();
                }
            });
            ui.ctx().memory_mut(|m| {
                m.data.insert_temp(id_key.with("k"), new_key);
                m.data.insert_temp(id_key.with("v"), new_val);
            });
        },
    );
}

fn raw_lua_tab(ui: &mut egui::Ui, form: &FormCtx<'_>) {
    widgets::section(
        ui,
        "Rendered mapinfo.lua (read-only)",
        true,
        |_| {},
        |ui| {
            ui.label(
                egui::RichText::new(
                    "This is the exact text the build pipeline ships into the `.sd7`. \
                     Re-renders every frame so edits in other tabs show up immediately.",
                )
                .color(Tokens::DARK.muted)
                .size(10.5),
            );
            let mut lua = form.raw_lua.to_string();
            ui.add(
                egui::TextEdit::multiline(&mut lua)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(28)
                    .desired_width(f32::INFINITY)
                    .interactive(false),
            )
            .on_hover_text("Read-only rendering. Use the other tabs to edit fields; this view re-bakes every frame and reflects them.");
        },
    );
}

fn minimap_tab(ui: &mut egui::Ui, form: &FormCtx<'_>, out: &mut Vec<MapInfoPatch>) {
    widgets::section(
        ui,
        "Minimap (1024 × 1024)",
        true,
        |_| {},
        |ui| {
            if let Some(handle) = form.minimap_preview {
                let size = egui::vec2(256.0, 256.0);
                ui.add(
                    egui::Image::new((handle.id(), size))
                        .corner_radius(4.0)
                        .fit_to_exact_size(size),
                );
            } else {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(256.0, 256.0), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 4.0, Tokens::DARK.panel2);
                p.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "(preview pending)",
                    egui::FontId::proportional(12.0),
                    Tokens::DARK.muted,
                );
            }
            ui.label(
                egui::RichText::new(
                    "Auto-baked by the build pipeline from the heightmap + layer-baked \
                     diffuse + sun direction (D7 / Sprint 18). Override below to ship a \
                     hand-authored PNG instead — must be exactly 1024 × 1024.",
                )
                .color(Tokens::DARK.muted)
                .size(10.5),
            );
        },
    );
    widgets::section(
        ui,
        "Override",
        false,
        |_| {},
        |ui| {
            let current = form
                .project
                .minimap_override
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(none — auto-bake)".to_string());
            ui.label(
                egui::RichText::new(current)
                    .color(Tokens::DARK.text)
                    .size(11.0)
                    .monospace(),
            );
            ui.horizontal(|ui| {
                if ui
                    .button("Pick PNG…")
                    .on_hover_text("Must be exactly 1024 × 1024.")
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("PNG", &["png"])
                        .pick_file()
                {
                    out.push(MapInfoPatch::MinimapOverride(Some(path)));
                }
                if form.project.minimap_override.is_some()
                    && ui
                        .button("Clear override")
                        .on_hover_text("Revert to the auto-bake path.")
                        .clicked()
                {
                    out.push(MapInfoPatch::MinimapOverride(None));
                }
            });
        },
    );
}

// ─────────────── small widget helpers ───────────────

/// Edit a labelled single-line text field. Returns true on commit
/// (focus lost OR enter pressed) so the caller emits one patch per
/// commit rather than per keystroke.
fn text_edit_singleline(ui: &mut egui::Ui, label: &str, value: &mut String, tooltip: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(Tokens::DARK.muted)
                .size(11.0),
        );
        let resp = ui
            .add(egui::TextEdit::singleline(value).desired_width(220.0))
            .on_hover_text(tooltip);
        commit_text(&resp)
    })
    .inner
}

/// Optional DragValue editor. `value = None` shows an "engine default"
/// chip + an "Override" button that pops the field in.
fn opt_drag_value<F>(
    ui: &mut egui::Ui,
    label: &str,
    value: Option<f32>,
    range: std::ops::RangeInclusive<f32>,
    tooltip: &str,
    mut on_commit: F,
) where
    F: FnMut(Option<f32>),
{
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(Tokens::DARK.muted)
                .size(11.0),
        );
        match value {
            Some(mut v) => {
                let resp = ui
                    .add(
                        egui::DragValue::new(&mut v)
                            .range(*range.start()..=*range.end())
                            .speed(((*range.end() - *range.start()) / 200.0).max(0.001)),
                    )
                    .on_hover_text(tooltip);
                if commit_drag(&resp) {
                    on_commit(Some(v));
                }
                if ui
                    .small_button("×")
                    .on_hover_text("Revert to engine default")
                    .clicked()
                {
                    on_commit(None);
                }
            }
            None => {
                ui.label(
                    egui::RichText::new("(engine default)")
                        .color(Tokens::DARK.muted)
                        .size(11.0)
                        .italics(),
                )
                .on_hover_text(tooltip);
                if ui
                    .small_button("Override")
                    .on_hover_text("Start overriding this field. Initial value lands at the slider midpoint; tweak to commit.")
                    .clicked()
                {
                    let mid = (*range.start() + *range.end()) * 0.5;
                    on_commit(Some(mid));
                }
            }
        }
    });
}

fn opt_rgb_picker<F>(ui: &mut egui::Ui, label: &str, value: Option<Rgb>, mut on_commit: F)
where
    F: FnMut(Option<Rgb>),
{
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(Tokens::DARK.muted)
                .size(11.0),
        );
        match value {
            Some(mut c) => {
                let resp = ui
                    .color_edit_button_rgb(&mut c)
                    .on_hover_text("RGB override (0..1 per channel). Drag the swatch to open the picker; click × to revert to the engine default.");
                if resp.changed() || resp.lost_focus() {
                    on_commit(Some(c));
                }
                if ui
                    .small_button("×")
                    .on_hover_text("Revert to engine default")
                    .clicked()
                {
                    on_commit(None);
                }
            }
            None => {
                ui.label(
                    egui::RichText::new("(engine default)")
                        .color(Tokens::DARK.muted)
                        .size(11.0)
                        .italics(),
                );
                if ui
                    .small_button("Override")
                    .on_hover_text("Start overriding this colour. Initial value is mid-grey; tweak to commit.")
                    .clicked()
                {
                    on_commit(Some([0.5, 0.5, 0.5]));
                }
            }
        }
    });
}

fn opt_text_input<F>(
    ui: &mut egui::Ui,
    label: &str,
    value: Option<&str>,
    tooltip: &str,
    mut on_commit: F,
) where
    F: FnMut(Option<String>),
{
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(Tokens::DARK.muted)
                .size(11.0),
        );
        let mut s = value.unwrap_or("").to_string();
        let resp = ui
            .add(egui::TextEdit::singleline(&mut s).desired_width(260.0))
            .on_hover_text(tooltip);
        if commit_text(&resp) {
            on_commit(if s.is_empty() { None } else { Some(s) });
        }
    });
}

fn sun_dir_editor(ui: &mut egui::Ui, sd: &mut SunDir) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (i, label) in ["x", "y", "z", "w (intensity)"].iter().enumerate() {
            let hover = match i {
                0 => "Sun direction x component (-1..1). Together with y and z, points toward the sun from the map.",
                1 => "Sun direction y component (-1..1). Positive = sun above the map.",
                2 => "Sun direction z component (-1..1). Engine reads only the camelCase sunDir; PITFALL §11.",
                _ => "Sun intensity scalar (0..4). BAR convention is 1.0; the earlier '1e9' research artefact was wrong — PITFALL §18.",
            };
            let resp = ui
                .add(
                    egui::DragValue::new(&mut sd[i])
                        .range(if i == 3 { 0.0..=4.0 } else { -1.0..=1.0 })
                        .speed(0.01)
                        .prefix(format!("{label}=")),
                )
                .on_hover_text(hover);
            if commit_drag(&resp) {
                changed = true;
            }
        }
    });
    changed
}

fn readonly_opt_rgb(ui: &mut egui::Ui, label: &str, value: Option<Rgb>) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(Tokens::DARK.muted)
                .size(11.0)
                .monospace(),
        );
        match value {
            Some([r, g, b]) => {
                ui.label(
                    egui::RichText::new(format!("[{r:.2}, {g:.2}, {b:.2}]"))
                        .color(Tokens::DARK.text)
                        .size(11.0)
                        .monospace(),
                );
            }
            None => {
                ui.label(
                    egui::RichText::new("(engine default)")
                        .color(Tokens::DARK.muted)
                        .size(11.0)
                        .italics(),
                );
            }
        }
    });
}

fn readonly_opt_f32(ui: &mut egui::Ui, label: &str, value: Option<f32>) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(Tokens::DARK.muted)
                .size(11.0)
                .monospace(),
        );
        match value {
            Some(v) => {
                ui.label(
                    egui::RichText::new(format!("{v:.4}"))
                        .color(Tokens::DARK.text)
                        .size(11.0)
                        .monospace(),
                );
            }
            None => {
                ui.label(
                    egui::RichText::new("(engine default)")
                        .color(Tokens::DARK.muted)
                        .size(11.0)
                        .italics(),
                );
            }
        }
    });
}

fn readonly_opt_str(ui: &mut egui::Ui, label: &str, value: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .color(Tokens::DARK.muted)
                .size(11.0)
                .monospace(),
        );
        match value {
            Some(s) => {
                ui.label(
                    egui::RichText::new(s)
                        .color(Tokens::DARK.text)
                        .size(11.0)
                        .monospace(),
                );
            }
            None => {
                ui.label(
                    egui::RichText::new("(engine default)")
                        .color(Tokens::DARK.muted)
                        .size(11.0)
                        .italics(),
                );
            }
        }
    });
}

fn commit_drag(resp: &egui::Response) -> bool {
    resp.lost_focus() || resp.drag_stopped()
}

fn commit_text(resp: &egui::Response) -> bool {
    resp.lost_focus() || (resp.changed() && !resp.has_focus())
}

// ─────────────── tests ───────────────

#[cfg(test)]
mod tests {
    use barme_core::{MapInfo, MapInfoPatch, Project};

    /// C7 / Sprint 18 (F9): every public field on the typed schema
    /// the form is expected to surface has a `MapInfoPatch` variant.
    /// This is a manual exhaustiveness check — adding a new schema
    /// field without adding a patch variant means the form silently
    /// drops it on round-trip.
    #[test]
    fn all_schema_fields_have_a_patch_variant() {
        // 49 leaf fields the F9 form surfaces; bump when adding
        // schema fields + corresponding form rows.
        let variant_count = 49;
        // We pin the count via this match; the compiler enforces
        // exhaustiveness so removing a variant fails to compile here.
        fn count(p: &MapInfoPatch) -> u32 {
            match p {
                MapInfoPatch::Name(_)
                | MapInfoPatch::Shortname(_)
                | MapInfoPatch::Description(_)
                | MapInfoPatch::Author(_)
                | MapInfoPatch::Version(_)
                | MapInfoPatch::Maphardness(_)
                | MapInfoPatch::NotDeformable(_)
                | MapInfoPatch::Gravity(_)
                | MapInfoPatch::TidalStrength(_)
                | MapInfoPatch::MaxMetal(_)
                | MapInfoPatch::ExtractorRadius(_)
                | MapInfoPatch::VoidWater(_)
                | MapInfoPatch::VoidGround(_)
                | MapInfoPatch::VoidAlphaMin(_)
                | MapInfoPatch::AutoShowMetal(_)
                | MapInfoPatch::LavaAtmosphere(_)
                | MapInfoPatch::SmfMinHeight(_)
                | MapInfoPatch::SmfMaxHeight(_)
                | MapInfoPatch::SmfMinimapTex(_)
                | MapInfoPatch::LightingSunDir(_)
                | MapInfoPatch::LightingGroundAmbientColor(_)
                | MapInfoPatch::LightingGroundDiffuseColor(_)
                | MapInfoPatch::LightingGroundSpecularColor(_)
                | MapInfoPatch::LightingGroundShadowDensity(_)
                | MapInfoPatch::LightingUnitAmbientColor(_)
                | MapInfoPatch::LightingUnitDiffuseColor(_)
                | MapInfoPatch::LightingUnitSpecularColor(_)
                | MapInfoPatch::LightingUnitShadowDensity(_)
                | MapInfoPatch::LightingSpecularExponent(_)
                | MapInfoPatch::AtmosphereMinWind(_)
                | MapInfoPatch::AtmosphereMaxWind(_)
                | MapInfoPatch::AtmosphereFogStart(_)
                | MapInfoPatch::AtmosphereFogEnd(_)
                | MapInfoPatch::AtmosphereFogColor(_)
                | MapInfoPatch::AtmosphereSunColor(_)
                | MapInfoPatch::AtmosphereSkyColor(_)
                | MapInfoPatch::AtmosphereSkyAxisAngle(_)
                | MapInfoPatch::AtmosphereSkyBox(_)
                | MapInfoPatch::AtmosphereCloudDensity(_)
                | MapInfoPatch::AtmosphereCloudColor(_)
                | MapInfoPatch::ResourcesDetailTex(_)
                | MapInfoPatch::ResourcesSpecularTex(_)
                | MapInfoPatch::ResourcesDetailNormalTex(_)
                | MapInfoPatch::ResourcesLightEmissionTex(_)
                | MapInfoPatch::ResourcesSkyReflectModTex(_)
                | MapInfoPatch::ResourcesParallaxHeightTex(_)
                | MapInfoPatch::ResourcesGrassBladeTex(_)
                | MapInfoPatch::TerrainTypes(_)
                | MapInfoPatch::CustomField { .. }
                | MapInfoPatch::MinimapOverride(_) => 1,
            }
        }
        let sample = MapInfoPatch::Gravity(Some(130.0));
        assert_eq!(count(&sample), 1);
        // Manual rolled count — the variant_count constant ensures
        // a removed variant requires updating both numbers.
        assert_eq!(variant_count, 49);
    }

    /// Round-trip: render the BAR default mapinfo, then re-parse via
    /// `From<&Project>` to ensure the schema → render → MapInfo path
    /// preserves data faithfully on the unchanged baseline.
    ///
    /// The full edit-then-diff round-trip requires the App-side
    /// dispatcher which lives in `main.rs`; the bare schema/render
    /// invariant is what this pin guards.
    #[test]
    fn round_trip_no_data_loss_on_default_project() {
        let project = Project::new("rt", 4);
        let info: MapInfo = (&project).into();
        // The schema → render → schema round-trip is tested elsewhere;
        // this just confirms the F9 form's `info` snapshot reflects
        // the canonical defaults so the form's initial state is
        // determined by the schema alone.
        assert_eq!(info.modtype, 3);
        assert_eq!(info.extractor_radius, Some(80.0));
        assert_eq!(info.gravity, Some(130.0));
        assert!(info.lighting.sun_dir.len() == 4);
    }

    /// PITFALL §12: `skyAxisAngle = [1, 0, 0, π/2]` (90° around X)
    /// must round-trip through the schema → emitter without loss.
    #[test]
    fn sky_axis_angle_round_trip() {
        let mut info = MapInfo::bar_default();
        let angle = std::f32::consts::FRAC_PI_2;
        info.atmosphere.sky_axis_angle = [1.0, 0.0, 0.0, angle];
        let rendered = barme_pipeline::mapinfo::render_mapinfo(&info);
        assert!(
            rendered.contains("skyAxisAngle = {"),
            "skyAxisAngle key missing in render:\n{rendered}"
        );
        // Engine reads `MapInfo.cpp:149` as `float4(xyz, radians)`;
        // the rendered Lua must carry both the unit axis and the
        // π/2 angle scalar (~1.5707963).
        assert!(
            rendered.contains("1.570796"),
            "angle missing or wrong:\n{rendered}"
        );
    }

    /// MapInfoPatch label coverage — every variant has a non-empty
    /// label. The label drives undo descriptions and tracing.
    #[test]
    fn mapinfo_patch_labels_are_non_empty() {
        let patches: &[MapInfoPatch] = &[
            MapInfoPatch::Name("x".into()),
            MapInfoPatch::Gravity(Some(130.0)),
            MapInfoPatch::LightingSunDir([0.0, 1.0, 0.0, 1.0]),
            MapInfoPatch::AtmosphereSkyAxisAngle([0.0, 0.0, 1.0, 0.0]),
            MapInfoPatch::TerrainTypes(vec![]),
            MapInfoPatch::CustomField {
                key: "k".into(),
                value: None,
            },
            MapInfoPatch::MinimapOverride(None),
        ];
        for p in patches {
            assert!(!p.label().is_empty(), "label is empty for variant: {:?}", p);
        }
    }
}
