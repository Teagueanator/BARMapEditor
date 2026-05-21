//! Sprint 25 / R1 / ADR-043 — renderer-parity fixture suite.
//!
//! The first fixture is **Comet Catcher Remake v1.8** by IceXuick
//! (original by NoiZe). The map exercises every Sprint-25 shader
//! feature: 4 distinct DNTS slot normals with per-channel UV scales,
//! a per-fragment specular texture, an R+A base normal map, and
//! `splatDetailNormalDiffuseAlpha = 1` (high-pass DNTS diffuse-in-
//! alpha). Mapinfo values are reproduced verbatim from
//! `scratch/bar-maps/extracted/comet/mapinfo.lua` (the extracted v1.8
//! archive in the user's local clone; same provenance as the
//! `assets/parity-fixtures/comet/README.md` smoke-procedure
//! documentation).
//!
//! The heightmap is synthesised. The real Comet `CCRXR.smf` is
//! gitignored (`*.smf` is in the global `.gitignore`); Sprint 36
//! (parity-validation) ships an SMF parser alongside the ΔE harness
//! it has to build anyway. Sprint 25's purpose is to lock the
//! fixture-loader API + the manual smoke procedure so the renderer-
//! parity arc has a stable target to iterate against.
//!
//! Lives in `src/`, NOT `tests/`, because `barme-app` is a
//! binary-only crate (no `lib.rs`) — integration-test files under
//! `tests/` can't link against its types. Tests run inline via
//! `#[cfg(test)]`. Future renderer-parity sprints can call
//! [`comet_catcher_fixture`] directly from anywhere in the app.

use crate::render::{SMF_INTENSITY_MULT, SplatUniforms};
use barme_core::{Heightmap, MapSize, Project, procgen};

/// Bundle returned by [`comet_catcher_fixture`]. Owns the project
/// (with its SMU dims + min/max height set), the synthesised
/// heightmap (Sprint 25 — see module-level comment for why),
/// and the splat uniforms suitable for `TerrainCallback::new`.
///
/// `project.heightmap` stays `None` — that field carries an on-disk
/// PNG path, not the in-memory `Heightmap` payload. The renderer
/// uploads `Heightmap::data()` via `render::upload_heightmap`.
#[allow(dead_code)] // Consumed by Sprint 36 / parity-validation harness; tests below.
pub struct CometFixture {
    pub project: Project,
    pub heightmap: Heightmap,
    pub splat: SplatUniforms,
}

/// Comet Catcher Remake v1.8 — the canonical Sprint 25 / R1 parity
/// fixture for the SMFFragProg port. Returns a `Project` whose shape
/// matches the map's mapinfo / SMF header + a `SplatUniforms` block
/// suitable for `TerrainCallback::new`.
///
/// Values reproduced verbatim from
/// `scratch/bar-maps/extracted/comet/mapinfo.lua`. The fixture is
/// deterministic — calling it twice from the same build produces
/// byte-identical `Project`s + `Heightmap`s.
///
/// # Heightmap source
///
/// Synthesised via `procgen::generate` because the real `CCRXR.smf`
/// is gitignored. The synthesis target is a height-range that
/// exercises both flat plateaus and sloped sections so the per-
/// fragment TBN + base-normal sampling get visible coverage at
/// editor-camera distance. Sprint 36 (parity-validation) will swap
/// this for a true SMF binary parser.
#[allow(dead_code)] // Consumed by Sprint 36 / parity-validation harness; tests below.
pub fn comet_catcher_fixture() -> CometFixture {
    // SMF header bytes from `xxd scratch/bar-maps/extracted/comet/maps/CCRXR.smf`:
    //   mapx = 1024, mapy = 768 → 16 × 12 SMU.
    let size = MapSize {
        smu_x: 16,
        smu_z: 12,
    };

    // Mapinfo `smf.minheight = 100`, `smf.maxheight = 450` — these
    // override the SMF-header values `(-50.0, 100.0)` per FINDINGS
    // §1.8 (`smf.{minHeight,maxHeight}` keys present ⇒ override).
    let min_height = 100.0_f32;
    let max_height = 450.0_f32;

    // Heightmap silhouette: a basin in the centre with ridges at
    // the perimeter — approximating Comet's "crater" shape. Domain
    // `Centered` lets x/z run in [-1, 1] so the radial distance is
    // straightforward. Sin/cos terms add ridge fingers so the slope
    // distribution isn't uniformly axisymmetric (a parabolic dome
    // would let aligned-axis bugs hide — good test for the per-
    // fragment TBN + normal sampling).
    let expr = "0.5 + 0.4 * math::sqrt(x*x + z*z) + 0.05 * math::sin(x*7) * math::cos(z*5)";
    let heightmap = procgen::generate(
        expr,
        procgen::Domain::Centered,
        size,
        min_height,
        max_height,
    )
    .expect("comet synthesised heightmap generates");

    // `Project::new` defaults to a square SMU — we override `size`
    // after construction to get 16×12. `heightmap` stays `None`
    // (the field is `Option<PathBuf>`, not in-memory data).
    let mut project = Project::new(
        "Comet Catcher Remake (Sprint 25 parity fixture)",
        size.smu_x,
    );
    project.size = size;
    project.min_height = min_height;
    project.max_height = max_height;
    // `Project.dnts_diffuse_in_alpha` mirrors the engine flag the
    // mapinfo's `resources.splatDetailNormalDiffuseAlpha` controls.
    project.dnts_diffuse_in_alpha = true;

    let splat = comet_splat_uniforms();

    CometFixture {
        project,
        heightmap,
        splat,
    }
}

/// Build the `SplatUniforms` block for the Comet fixture. Lighting
/// values come from `mapinfo.lighting`; splat scales / mults from
/// `mapinfo.splats`; the texture-presence bitfield reflects that the
/// real Comet archive ships a `normalmap.png`, `specular.png`, and 4
/// DNTS slot normals. The `diffuse_in_alpha` bit is set because Comet
/// ships `splatDetailNormalDiffuseAlpha = 1`.
///
/// `ground_ambient` + `ground_diffuse` are pre-multiplied by
/// `SMF_INTENSITY_MULT = 210/255` CPU-side per FINDINGS §7.1. The
/// shader does NOT re-apply the dim — it consumes these values
/// directly.
#[allow(dead_code)] // Indirectly consumed via comet_catcher_fixture.
fn comet_splat_uniforms() -> SplatUniforms {
    // splats.TexScales = { 0.004, 0.007, 0.003, 0.0018 }
    let tex_scales = [0.004, 0.007, 0.003, 0.0018];
    // splats.TexMults = { 0.4, 0.4, 0.65, 0.9 }
    let tex_mults = [0.4, 0.4, 0.65, 0.9];

    // lighting.sunDir = { 1.2, 0.92, -0.79 }; we normalise because
    // the WGSL expects a unit vector. The engine reads `lightDir.w`
    // as an intensity scalar; we keep it at 1.0 per FINDINGS §1.4 /
    // PITFALL §18.
    let sun = glam::Vec3::new(1.2, 0.92, -0.79).normalize_or_zero();

    // lighting.groundAmbientColor = { 0.55, 0.51, 0.51 }
    // FINDINGS §7.1 — pre-multiply by SMF_INTENSITY_MULT.
    let amb = [
        0.55 * SMF_INTENSITY_MULT,
        0.51 * SMF_INTENSITY_MULT,
        0.51 * SMF_INTENSITY_MULT,
        0.0,
    ];
    // lighting.groundDiffuseColor = { 1, 1, 1 }
    let dif = [
        SMF_INTENSITY_MULT,
        SMF_INTENSITY_MULT,
        SMF_INTENSITY_MULT,
        0.0,
    ];
    // lighting.groundSpecularColor = { 0.5, 0.5, 0.5 }. Comet's
    // mapinfo.lighting block doesn't carry an explicit
    // `specularExponent`, so we keep the engine default `100.0`
    // (FINDINGS §1.4). The shader uses this fallback only when no
    // specular texture is bound; Comet ships `specularTex` so the
    // per-fragment `α × 16.0` path dominates.
    let spec = [0.5, 0.5, 0.5, 100.0];

    // Active slot mask — Comet binds all 4 DNTS slots.
    let active_slot_mask: u32 = 0b1111;
    // diffuse_in_alpha = 1 — Comet ships `splatDetailNormalDiffuseAlpha = 1`.
    let diffuse_in_alpha: u32 = 1;
    // buildable overlay off in the fixture (user toggles at runtime).
    let buildable_overlay_on: u32 = 0;
    // Texture-presence bitfield (Sprint 25 / R1 / ADR-043):
    //   bit 0 = has_base_normal_tex   — Comet ships normalmap.png
    //   bit 1 = has_specular_tex      — Comet ships specular.png
    //   bit 2 = has_dnts_slot_normals — Comet ships 4 DNTS TGAs
    let tex_present_bits: u32 = 0b111;

    SplatUniforms {
        tex_scales,
        tex_mults,
        flags: [
            active_slot_mask,
            diffuse_in_alpha,
            buildable_overlay_on,
            tex_present_bits,
        ],
        sun_dir: [sun.x, sun.y, sun.z, 1.0],
        ground_ambient: amb,
        ground_diffuse: dif,
        ground_specular: spec,
        // camera_pos lands per-frame via `TerrainCallback::prepare`.
        camera_pos: [0.0, 0.0, 0.0, 1.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comet_fixture_dims_match_smf_header() {
        let f = comet_catcher_fixture();
        // SMF header: mapx = 1024, mapy = 768 → 16 × 12 SMU.
        assert_eq!(f.project.size.smu_x, 16);
        assert_eq!(f.project.size.smu_z, 12);
        // Heightmap edge length is `64·N + 1`.
        let (w, h) = f.project.size.heightmap_dims();
        assert_eq!((w, h), (1025, 769));
        let (hw, hh) = f.heightmap.dims();
        assert_eq!((hw, hh), (1025, 769), "synth heightmap dim must match SMU");
    }

    #[test]
    fn comet_fixture_smf_overrides_min_max_height() {
        // mapinfo.smf.minheight = 100, mapinfo.smf.maxheight = 450 —
        // overrides the SMF header's (-50, 100). FINDINGS §1.8.
        let f = comet_catcher_fixture();
        assert!((f.project.min_height - 100.0).abs() < 1e-6);
        assert!((f.project.max_height - 450.0).abs() < 1e-6);
    }

    #[test]
    fn comet_fixture_splat_scales_match_mapinfo() {
        // mapinfo.splats.TexScales = { 0.004, 0.007, 0.003, 0.0018 }
        let f = comet_catcher_fixture();
        assert_eq!(f.splat.tex_scales, [0.004, 0.007, 0.003, 0.0018]);
    }

    #[test]
    fn comet_fixture_splat_mults_match_mapinfo() {
        // mapinfo.splats.TexMults = { 0.4, 0.4, 0.65, 0.9 }
        let f = comet_catcher_fixture();
        assert_eq!(f.splat.tex_mults, [0.4, 0.4, 0.65, 0.9]);
    }

    #[test]
    fn comet_fixture_sun_dir_normalised() {
        // mapinfo.lighting.sunDir = { 1.2, 0.92, -0.79 } — un-normalised
        // in the Lua. The shader expects a unit vector, so the fixture
        // normalises CPU-side. `.w = 1.0` per PITFALL §18 / FINDINGS §1.4.
        let f = comet_catcher_fixture();
        let m =
            (f.splat.sun_dir[0].powi(2) + f.splat.sun_dir[1].powi(2) + f.splat.sun_dir[2].powi(2))
                .sqrt();
        assert!((m - 1.0).abs() < 1e-6, "sun_dir not unit length: |m| = {m}");
        assert!((f.splat.sun_dir[3] - 1.0).abs() < 1e-6, ".w should be 1.0");
        // Direction sign: x > 0, y > 0, z < 0.
        assert!(f.splat.sun_dir[0] > 0.0);
        assert!(f.splat.sun_dir[1] > 0.0);
        assert!(f.splat.sun_dir[2] < 0.0);
    }

    #[test]
    fn comet_fixture_ambient_pre_dimmed_by_intensity_mult() {
        // FINDINGS §7.1 — `SMF_INTENSITY_MULT = 210/255` is applied to
        // ambient + diffuse CPU-side. Comet's mapinfo.groundAmbientColor
        // is (0.55, 0.51, 0.51); the fixture's pre-dimmed values should
        // be those × 210/255.
        let f = comet_catcher_fixture();
        let r_expected = 0.55 * SMF_INTENSITY_MULT;
        let g_expected = 0.51 * SMF_INTENSITY_MULT;
        let b_expected = 0.51 * SMF_INTENSITY_MULT;
        assert!((f.splat.ground_ambient[0] - r_expected).abs() < 1e-6);
        assert!((f.splat.ground_ambient[1] - g_expected).abs() < 1e-6);
        assert!((f.splat.ground_ambient[2] - b_expected).abs() < 1e-6);
    }

    #[test]
    fn comet_fixture_diffuse_pre_dimmed_by_intensity_mult() {
        // Comet's mapinfo.groundDiffuseColor = (1, 1, 1); the pre-dimmed
        // values should equal SMF_INTENSITY_MULT exactly.
        let f = comet_catcher_fixture();
        for c in &f.splat.ground_diffuse[..3] {
            assert!((c - SMF_INTENSITY_MULT).abs() < 1e-6, "got {c}");
        }
    }

    #[test]
    fn comet_fixture_specular_color_matches_mapinfo() {
        // Comet's mapinfo.groundSpecularColor = (0.5, 0.5, 0.5). The
        // exponent default is 100.0 (engine default — Comet doesn't
        // override `specularExponent`). The shader only consults these
        // when no specular texture is bound; pinned here so a fixture
        // edit gets reviewed.
        let f = comet_catcher_fixture();
        assert_eq!(f.splat.ground_specular, [0.5, 0.5, 0.5, 100.0]);
    }

    #[test]
    fn comet_fixture_texture_presence_bits_match_archive_shape() {
        // Comet ships normalmap.png + specular.png + 4 DNTS TGAs, so
        // all three Sprint 25 / R1 / ADR-043 presence bits are set.
        let f = comet_catcher_fixture();
        let bits = f.splat.flags[3];
        assert_eq!(bits & 1, 1, "has_base_normal_tex bit");
        assert_eq!((bits >> 1) & 1, 1, "has_specular_tex bit");
        assert_eq!((bits >> 2) & 1, 1, "has_dnts_slot_normals bit");
    }

    #[test]
    fn comet_fixture_diffuse_in_alpha_set() {
        // mapinfo.resources.splatDetailNormalDiffuseAlpha = 1. This
        // gates the WGSL's `splat_detail_strength.y` add to diffuse
        // (FINDINGS §7.3 / SMFFragProg.glsl:192).
        let f = comet_catcher_fixture();
        assert_eq!(f.splat.flags[1], 1);
    }

    #[test]
    fn comet_fixture_active_slot_mask_covers_all_four_channels() {
        // Comet binds splatDetailNormalTex1..4 — all four channels live.
        let f = comet_catcher_fixture();
        assert_eq!(f.splat.flags[0], 0b1111);
    }

    #[test]
    fn comet_fixture_heightmap_data_spans_min_max_range() {
        // Sanity: the synth heightmap actually exercises the full
        // [min_h, max_h] band. If the expression accidentally renders
        // a flat surface, the per-fragment TBN path doesn't get
        // coverage.
        let f = comet_catcher_fixture();
        let data = f.heightmap.data();
        let min_raw = *data.iter().min().unwrap();
        let max_raw = *data.iter().max().unwrap();
        // Each `u16` linearly maps [0, 65535] → [min_h, max_h].
        // Require the synth to use at least 50 % of the dynamic
        // range — anything less suggests the procgen expression
        // collapsed.
        let range_used = (max_raw as u32) - (min_raw as u32);
        assert!(
            range_used > 32_000,
            "synth heightmap range too narrow: {range_used} of 65535"
        );
    }

    #[test]
    fn comet_fixture_project_dnts_diffuse_in_alpha_set() {
        // The fixture flips `Project.dnts_diffuse_in_alpha = true` to
        // mirror Comet's mapinfo `splatDetailNormalDiffuseAlpha = 1`.
        // App code reads `Project.dnts_diffuse_in_alpha` into the
        // SplatUniforms.flags.y bit through `splat_uniforms_for_render`;
        // pinning the project flag here keeps that pipeline honest
        // even when the renderer's splat-uniforms helper changes.
        let f = comet_catcher_fixture();
        assert!(f.project.dnts_diffuse_in_alpha);
    }
}
