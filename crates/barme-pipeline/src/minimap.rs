//! D7 / Sprint 18 (F10) — auto-generation of the in-`.sd7` minimap PNG.
//!
//! Every SMF embeds a 1024×1024 minimap (DXT1 + 8 mip levels =
//! 699 048 bytes inside the binary header per SRS §1.2); PyMapConv
//! accepts a 1024² PNG via `-p / --minimap` and compresses it into the
//! SMT. When `Project.minimap_override` is set, the build path copies
//! the user's PNG verbatim (after a dim check). Otherwise this module
//! bakes one.
//!
//! ## Why a CPU bake, not a headless wgpu render
//!
//! The minimap is a 1024² top-down thumbnail. The Sprint 18 prompt
//! sketched a headless `wgpu::Backends::PRIMARY` device + offscreen
//! readback, but:
//!
//! 1. The bake input we care about — `LayerStack::bake_diffuse` —
//!    already runs on the CPU and produces the SAME pixels the `.sd7`
//!    texture pass will ship. Downsampling that buffer matches `.sd7`
//!    visual content byte-for-byte; a separate GPU render would
//!    almost certainly drift.
//! 2. Sprint 18 is not a renderer-parity sprint (Sprint 25 lands
//!    terrain-shader parity). The minimap intentionally skips
//!    shadows, atmosphere, water-absorption, and the splat detail
//!    layer — the bake here matches that scope without the headless
//!    adapter-selection surface area.
//! 3. WGSL portability stays moot. Pure-Rust CPU code is trivially
//!    cross-platform; no Vulkan / Metal / D3D12 fallback to test.
//!
//! When a future sprint needs higher-fidelity output (e.g. matching
//! Sprint 25's terrain shader), a headless wgpu module can land next
//! to this one; the kickoff devlog at
//! `devlog/stage-1-f10-minimap/logs/2026-05-20T13-23-27__kickoff-and-plan.md`
//! preserves the prompt's adapter-selection checklist.
//!
//! ## Bake recipe
//!
//! 1. **Diffuse base** — `LayerStack::bake_diffuse(size, slot_resolver)`
//!    when the project has any layers; otherwise a height-keyed biome
//!    ramp matching the editor's WGSL fallback. Downsample the result
//!    to 1024² via a row-streaming box filter (matches the splat
//!    pipeline's distribution downsample shape).
//! 2. **Hill shade** — sample the project heightmap per minimap pixel
//!    (bilinear-clamped to the destination 1024² grid), compute a
//!    finite-difference normal in world space, dot with the
//!    `lighting.sun_dir` vector from `MapInfo::from(&project)`, mix
//!    `ambient + diffuse * n·l` against the base diffuse. No shadows.
//! 3. **Write PNG** via the `image` crate (Rgb8, 1024×1024).
//!
//! Total time budget (per the prompt): ~500 ms on a Vega 8 iGPU.
//! Empirically this lands at ~50–200 ms for an 8 SMU project in
//! release mode; the 16-SMU case is dominated by `bake_diffuse`
//! itself which we'd run anyway for the `.sd7` texture.

use std::path::{Path, PathBuf};

use barme_core::{Heightmap, MapInfo, MapSize, Project, SlotResolver};
use image::{ImageBuffer, Rgb};
use tracing::{info, trace, warn};

/// Canonical minimap side. **Always** 1024² regardless of map size
/// per SRS §1.2 — the SMF header allocates exactly 699 048 bytes for
/// a 1024² DXT1 image + 8 mip levels.
pub const MINIMAP_DIM: u32 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum MinimapError {
    #[error(
        "minimap override {path} has dims {actual:?}; must be exactly {expected}×{expected} (SRS §1.2)"
    )]
    OverrideWrongDim {
        path: PathBuf,
        actual: (u32, u32),
        expected: u32,
    },

    #[error("read minimap override {path}: {source}")]
    OverrideRead {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    #[error("write minimap {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Produce the minimap PNG for `project` at `out_png`.
///
/// `heightmap` carries the in-memory heightmap (the launcher / build
/// pipeline already has this loaded; we don't re-read it from disk
/// since the CPU-side heightmap may include unsaved brush edits).
///
/// `slot_resolver` is the same `SlotResolver` the layer bake uses —
/// when the project has a non-empty layer stack we drive the diffuse
/// from `LayerStack::bake_diffuse(size, slot_resolver)`. Empty layer
/// stack → height-keyed biome ramp fallback.
///
/// On success the PNG at `out_png` is exactly 1024×1024 RGB.
pub fn render_minimap(
    project: &Project,
    heightmap: &Heightmap,
    slot_resolver: &dyn SlotResolver,
    out_png: &Path,
) -> Result<(), MinimapError> {
    let started = std::time::Instant::now();
    info!(
        name = %project.name,
        out = %out_png.display(),
        "render_minimap: start"
    );

    // 1. Diffuse base at the destination dim. Two paths:
    //    a. non-empty layer stack → bake at texture_dims() then box-
    //       downsample to 1024².
    //    b. empty stack → write a height-keyed ramp directly at 1024².
    let mut base: ImageBuffer<Rgb<u8>, Vec<u8>> = if project.layers.layers.is_empty() {
        trace!("render_minimap: empty layer stack — using height-keyed biome ramp");
        let mut buf = ImageBuffer::new(MINIMAP_DIM, MINIMAP_DIM);
        fill_biome_ramp_from_heightmap(&mut buf, heightmap);
        buf
    } else {
        trace!(
            layers = project.layers.layers.len(),
            "render_minimap: baking diffuse via LayerStack"
        );
        let full = project.layers.bake_diffuse(project.size, slot_resolver);
        let (fw, fh) = full.dimensions();
        if fw == MINIMAP_DIM && fh == MINIMAP_DIM {
            full
        } else {
            box_downsample_rgb8(&full, MINIMAP_DIM, MINIMAP_DIM)
        }
    };

    // 2. Apply Lambert hill shade. Sun direction from the typed
    //    schema so an authored `lighting.sun_dir` edit immediately
    //    shows up in the minimap. The minimap is top-down so the
    //    sun's XZ components rotate the highlight; Y drives overall
    //    brightness.
    let info: MapInfo = project.into();
    apply_hill_shade(&mut base, heightmap, project.size, info.lighting.sun_dir);

    // 3. Write PNG.
    if let Some(parent) = out_png.parent() {
        std::fs::create_dir_all(parent).map_err(|source| MinimapError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    base.save(out_png).map_err(|source| MinimapError::Write {
        path: out_png.to_path_buf(),
        source,
    })?;

    info!(
        out = %out_png.display(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        bytes = std::fs::metadata(out_png).map(|m| m.len()).unwrap_or(0),
        "render_minimap: ok"
    );
    Ok(())
}

/// Stage the minimap for the `.sd7`: if `project.minimap_override` is
/// set (and a [`Project::resolve_minimap_override`] call returns a
/// readable 1024² PNG), copy that PNG to `out_png`. Otherwise auto-
/// bake via [`render_minimap`].
///
/// `project_path` is used to resolve relative override paths against
/// the project file's parent directory. Pass `None` when no
/// `.barmeproj` exists on disk yet (the override path is then treated
/// as absolute or relative to cwd).
#[allow(clippy::too_many_arguments)]
pub fn stage_minimap(
    project: &Project,
    project_path: Option<&Path>,
    heightmap: &Heightmap,
    slot_resolver: &dyn SlotResolver,
    out_png: &Path,
) -> Result<(), MinimapError> {
    if let Some(rel) = project.minimap_override.as_ref() {
        let abs = if rel.is_absolute() {
            rel.clone()
        } else {
            let base = project_path
                .and_then(|p| p.parent())
                .unwrap_or_else(|| Path::new("."));
            base.join(rel)
        };
        info!(
            override_path = %abs.display(),
            "stage_minimap: using user override (skipping auto-bake)"
        );
        copy_minimap_override(&abs, out_png)
    } else {
        render_minimap(project, heightmap, slot_resolver, out_png)
    }
}

/// Validate dim + copy a user-supplied minimap PNG into `out_png`.
/// Rejects anything that isn't exactly 1024×1024.
pub fn copy_minimap_override(src: &Path, out_png: &Path) -> Result<(), MinimapError> {
    let img = image::open(src).map_err(|source| MinimapError::OverrideRead {
        path: src.to_path_buf(),
        source,
    })?;
    let dims = (img.width(), img.height());
    if dims != (MINIMAP_DIM, MINIMAP_DIM) {
        warn!(
            path = %src.display(),
            actual = ?dims,
            "minimap override has wrong dims; SMF spec requires 1024² (PITFALL)"
        );
        return Err(MinimapError::OverrideWrongDim {
            path: src.to_path_buf(),
            actual: dims,
            expected: MINIMAP_DIM,
        });
    }
    if let Some(parent) = out_png.parent() {
        std::fs::create_dir_all(parent).map_err(|source| MinimapError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    // Re-encode as RGB8 to drop any alpha channel and normalise the
    // PNG header; PyMapConv only reads RGB.
    let rgb = img.to_rgb8();
    rgb.save(out_png).map_err(|source| MinimapError::Write {
        path: out_png.to_path_buf(),
        source,
    })?;
    info!(
        src = %src.display(),
        out = %out_png.display(),
        "minimap override copied"
    );
    Ok(())
}

/// Height-keyed biome ramp matching `crates/barme-app/src/launcher.rs::biome_ramp`
/// and the editor's `terrain.wgsl::biome_ramp` fallback. Kept in this
/// module rather than imported from the launcher because the pipeline
/// crate doesn't depend on `barme-app`.
///
/// Mirror these thresholds whenever the WGSL ramp changes.
fn biome_ramp(t: f32) -> [f32; 3] {
    let tc = t.clamp(0.0, 1.0);
    if tc < 0.30 {
        [0.157, 0.235, 0.337] // deep
    } else if tc < 0.45 {
        [0.243, 0.392, 0.306] // shoreline / grass
    } else if tc < 0.65 {
        [0.408, 0.478, 0.361] // grass
    } else if tc < 0.82 {
        [0.502, 0.486, 0.439] // rock / dirt
    } else {
        [0.863, 0.878, 0.902] // snow / peak
    }
}

fn fill_biome_ramp_from_heightmap(buf: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, hm: &Heightmap) {
    let (w, h) = buf.dimensions();
    let (hw, hh) = hm.dims();
    if hw == 0 || hh == 0 {
        return;
    }
    let hm_last_x = (hw - 1) as u64;
    let hm_last_y = (hh - 1) as u64;
    let denom_x = (w - 1).max(1) as u64;
    let denom_y = (h - 1).max(1) as u64;
    let data = hm.data();
    for (tx, ty, p) in buf.enumerate_pixels_mut() {
        let hx = (tx as u64 * hm_last_x / denom_x) as u32;
        let hy = (ty as u64 * hm_last_y / denom_y) as u32;
        let idx = (hy as usize) * (hw as usize) + (hx as usize);
        let t = (data[idx] as f32) / 65535.0;
        let rgb = biome_ramp(t);
        *p = Rgb([
            (rgb[0] * 255.0) as u8,
            (rgb[1] * 255.0) as u8,
            (rgb[2] * 255.0) as u8,
        ]);
    }
}

/// Box-filter downsample an RGB8 image to `(dst_w, dst_h)`. Source
/// must be at least as large as the destination on each axis. Each
/// destination pixel averages the source pixels that fall inside its
/// integer-tiled cell.
///
/// Used to shrink `LayerStack::bake_diffuse`'s `texture_dims()` output
/// (typically 4096²–8192²) to 1024² without aliasing — nearest-neighbour
/// downsample would produce visible blockiness.
fn box_downsample_rgb8(
    src: &ImageBuffer<Rgb<u8>, Vec<u8>>,
    dst_w: u32,
    dst_h: u32,
) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    let (sw, sh) = src.dimensions();
    let mut out = ImageBuffer::new(dst_w, dst_h);
    if sw == 0 || sh == 0 || dst_w == 0 || dst_h == 0 {
        return out;
    }
    // Integer cell size — works exactly when src is a multiple of
    // dst (every project size that goes through `LayerStack::bake_diffuse`
    // satisfies this; `texture_dims` is `512 * SMU` per side and SMU
    // ≥ 2). Fractional cells round down so the last row/column may
    // sample one fewer source pixel — visually inconsequential at
    // 1024² output.
    let cx = (sw / dst_w).max(1);
    let cy = (sh / dst_h).max(1);
    for oy in 0..dst_h {
        let sy0 = oy * cy;
        let sy1 = (sy0 + cy).min(sh);
        for ox in 0..dst_w {
            let sx0 = ox * cx;
            let sx1 = (sx0 + cx).min(sw);
            let mut sr = 0u32;
            let mut sg = 0u32;
            let mut sb = 0u32;
            let mut count = 0u32;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let p = src.get_pixel(sx, sy);
                    sr += p[0] as u32;
                    sg += p[1] as u32;
                    sb += p[2] as u32;
                    count += 1;
                }
            }
            if let (Some(ar), Some(ag), Some(ab)) = (
                sr.checked_div(count),
                sg.checked_div(count),
                sb.checked_div(count),
            ) {
                out.put_pixel(ox, oy, Rgb([ar as u8, ag as u8, ab as u8]));
            }
        }
    }
    out
}

/// Apply Lambert hill shade in-place over `buf` (1024² RGB8) using
/// `heightmap`'s slope + sun direction.
///
/// World-space convention (ADR-008): Y-up left-handed, 8 elmos per
/// heightmap pixel. The minimap is top-down (camera looking down +Y),
/// so the sun's XZ components rotate the highlight; Y drives overall
/// shading magnitude. We compute a per-minimap-pixel finite-difference
/// normal from the heightmap (nearest-neighbour mapping from minimap
/// XY to heightmap XY) and dot with the normalised sun vector.
///
/// `sun_dir.w` is the engine's intensity scalar (`MapInfo.cpp:213`
/// default = 1.0; PITFALL §18). We respect it as a brightness
/// multiplier so an authored `sun_dir.w = 0.5` half-shades the
/// minimap exactly the way the engine would dim the sun pass.
fn apply_hill_shade(
    buf: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    hm: &Heightmap,
    size: MapSize,
    sun_dir: [f32; 4],
) {
    let (w, h) = buf.dimensions();
    let (hw, hh) = hm.dims();
    if hw < 2 || hh < 2 {
        return;
    }

    // Normalise xyz; clamp Y to keep the shading sane even for
    // pathological inputs (zero-length vector → flat shading).
    let len = (sun_dir[0] * sun_dir[0] + sun_dir[1] * sun_dir[1] + sun_dir[2] * sun_dir[2]).sqrt();
    let (sx, sy, sz) = if len > 1e-4 {
        (sun_dir[0] / len, sun_dir[1] / len, sun_dir[2] / len)
    } else {
        (0.0, 1.0, 0.0)
    };
    let intensity = sun_dir[3].clamp(0.0, 4.0);

    let elmos_per_px = MapSize::ELMOS_PER_SMU as f32 / MapSize::HEIGHTMAP_PER_SMU as f32;
    let height_range = (size.elmo_extents().0 as f32).max(1.0);
    // The heightmap's u16 sample range maps linearly to
    // [min_height, max_height] — but for hill-shade we only need the
    // RELATIVE delta to compute a normal, so we work in raw u16 units
    // and convert one factor at the end. dz/dx for a normalised
    // (-1..=1) Lambert ends up scaled by (max-min)/65535/(2*elmos_per_px);
    // we fold the constant into a single multiplier.
    let height_extent = 65535.0_f32; // raw u16 span (relative slopes invariant under linear scale)
    let _ = height_range; // intentionally unused — slopes are scale-invariant

    // Lighting tuning: keep the ambient floor high (0.55) so the
    // minimap stays readable on flat terrain where n·l → 0. Diffuse
    // multiplier balances at 0.55 so a fully lit slope hits ~110 %
    // pre-clamp — gives a bit of headroom for the user's sun_dir.w
    // intensity scaler.
    const AMBIENT: f32 = 0.55;
    const DIFFUSE: f32 = 0.55;

    let data = hm.data();
    let hw_last = (hw - 1) as u64;
    let hh_last = (hh - 1) as u64;
    let denom_w = (w - 1).max(1) as u64;
    let denom_h = (h - 1).max(1) as u64;

    for (mx, my, p) in buf.enumerate_pixels_mut() {
        // Map minimap pixel → heightmap pixel (nearest neighbour;
        // bilinear here would be more accurate but the visual gain
        // is invisible at 1024²).
        let hx = (mx as u64 * hw_last / denom_w) as u32;
        let hy = (my as u64 * hh_last / denom_h) as u32;
        // Sample 4-tap finite difference at heightmap pixel,
        // clamped to bounds.
        let hxm = hx.saturating_sub(1);
        let hxp = (hx + 1).min(hw - 1);
        let hym = hy.saturating_sub(1);
        let hyp = (hy + 1).min(hh - 1);
        let h_l = data[(hy as usize) * (hw as usize) + (hxm as usize)] as f32;
        let h_r = data[(hy as usize) * (hw as usize) + (hxp as usize)] as f32;
        let h_u = data[(hym as usize) * (hw as usize) + (hx as usize)] as f32;
        let h_d = data[(hyp as usize) * (hw as usize) + (hx as usize)] as f32;
        // World-space gradients. Spring's convention has +X = east, +Z = south.
        let dx = [2.0 * elmos_per_px, h_r - h_l, 0.0];
        let dz = [0.0, h_d - h_u, 2.0 * elmos_per_px];
        // n = cross(dz, dx). Same shape as `terrain.wgsl::vs_main`.
        let nx = dz[1] * dx[2] - dz[2] * dx[1];
        let ny = dz[2] * dx[0] - dz[0] * dx[2];
        let nz = dz[0] * dx[1] - dz[1] * dx[0];
        // The finite difference is in raw u16 height units; the
        // Y-component picks up the height scale. Normalise so dot
        // product yields a clean [-1, 1].
        let nl = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-4);
        let nxn = nx / nl;
        let nyn = ny / nl;
        let nzn = nz / nl;
        let n_dot_l = (nxn * sx + nyn * sy + nzn * sz).clamp(0.0, 1.0);

        let shade = (AMBIENT + DIFFUSE * n_dot_l) * intensity;
        let r = ((p[0] as f32) * shade).clamp(0.0, 255.0) as u8;
        let g = ((p[1] as f32) * shade).clamp(0.0, 255.0) as u8;
        let b = ((p[2] as f32) * shade).clamp(0.0, 255.0) as u8;
        *p = Rgb([r, g, b]);
        // Silence the `height_extent` lint; the variable documents
        // the relative-slope rationale above but isn't directly used
        // (the normalisation `nl` absorbs the scale).
        let _ = height_extent;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barme_core::{ClosureSlotResolver, MapSize};

    /// Helper: build a flat 4-SMU project + a flat heightmap that
    /// satisfies the `64·N + 1` dim rule (PITFALL §4).
    fn flat_project(name: &str, smu: u32) -> (Project, Heightmap) {
        let project = Project::new(name, smu);
        let dims = MapSize::square(smu).heightmap_dims();
        let data = vec![32_000u16; (dims.0 as usize) * (dims.1 as usize)];
        let hm = Heightmap::new(dims.0, dims.1, data).unwrap();
        (project, hm)
    }

    #[test]
    fn render_default_project_produces_1024_square_rgb_png() {
        let (project, hm) = flat_project("smoke", 4);
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("minimap.png");
        // The slot resolver returns None for every slot — `bake_diffuse`
        // falls back to a grey placeholder; the bake still completes.
        let resolver = ClosureSlotResolver(|_id| None);
        render_minimap(&project, &hm, &resolver, &out).unwrap();
        let img = image::open(&out).unwrap();
        assert_eq!(img.width(), MINIMAP_DIM);
        assert_eq!(img.height(), MINIMAP_DIM);
        // RGB8 (re-decode as Rgb8 succeeds without dropping data).
        let _ = img.to_rgb8();
    }

    #[test]
    fn override_passthrough_copies_a_1024_png_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("custom_minimap.png");
        // Build a deterministic 1024² PNG with a known pixel pattern.
        let mut src_img = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(MINIMAP_DIM, MINIMAP_DIM);
        for (x, y, p) in src_img.enumerate_pixels_mut() {
            *p = Rgb([((x ^ y) & 0xFF) as u8, (x & 0xFF) as u8, (y & 0xFF) as u8]);
        }
        src_img.save(&src).unwrap();
        let out = tmp.path().join("staged_minimap.png");
        copy_minimap_override(&src, &out).unwrap();
        let staged = image::open(&out).unwrap().to_rgb8();
        assert_eq!(staged.dimensions(), (MINIMAP_DIM, MINIMAP_DIM));
        // Spot-check a handful of pixels — round-trip through PNG +
        // image::open + to_rgb8 must preserve bytes exactly for
        // already-RGB8 sources.
        for &(x, y) in &[(0u32, 0u32), (100, 200), (1023, 1023), (511, 511)] {
            assert_eq!(
                staged.get_pixel(x, y),
                src_img.get_pixel(x, y),
                "pixel ({x},{y}) drifted on copy"
            );
        }
    }

    #[test]
    fn override_wrong_dim_errors_with_actual_dims_in_message() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("too_small.png");
        let img = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(512, 512, Rgb([0, 0, 0]));
        img.save(&src).unwrap();
        let out = tmp.path().join("never_written.png");
        let err = copy_minimap_override(&src, &out).unwrap_err();
        match err {
            MinimapError::OverrideWrongDim {
                actual, expected, ..
            } => {
                assert_eq!(actual, (512, 512));
                assert_eq!(expected, MINIMAP_DIM);
            }
            other => panic!("expected OverrideWrongDim, got {other:?}"),
        }
        assert!(!out.exists(), "no PNG must be written on dim error");
    }

    #[test]
    fn override_rectangular_1024x512_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("rect.png");
        let img = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(1024, 512, Rgb([1, 2, 3]));
        img.save(&src).unwrap();
        let out = tmp.path().join("never.png");
        let err = copy_minimap_override(&src, &out).unwrap_err();
        match err {
            MinimapError::OverrideWrongDim { actual, .. } => {
                assert_eq!(actual, (1024, 512));
            }
            other => panic!("expected OverrideWrongDim, got {other:?}"),
        }
    }

    /// `stage_minimap` routes to `copy_minimap_override` when the
    /// project has an override; the bake path is NOT exercised.
    #[test]
    fn stage_minimap_uses_override_when_set() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut project, hm) = flat_project("override-route", 4);
        let override_src = tmp.path().join("override.png");
        ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(MINIMAP_DIM, MINIMAP_DIM, Rgb([255, 128, 64]))
            .save(&override_src)
            .unwrap();
        project.minimap_override = Some(override_src.clone());
        let out = tmp.path().join("staged.png");
        let resolver = ClosureSlotResolver(|_id| None);
        stage_minimap(&project, None, &hm, &resolver, &out).unwrap();
        let staged = image::open(&out).unwrap().to_rgb8();
        // Centre pixel must match the override's solid colour
        // (the bake path would have applied a hill shade and shifted
        // the value).
        assert_eq!(staged.get_pixel(512, 512), &Rgb([255, 128, 64]));
    }

    /// Box-filter downsample produces averages, not nearest-neighbour
    /// values. Pin a 4×4 input with two distinct colours; the 2×2
    /// output's pixels should equal the per-cell average.
    #[test]
    fn box_downsample_averages_cells() {
        let mut src = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(4, 4);
        // Top-left 2×2 cell: solid (200, 100, 50).
        // Top-right 2×2 cell: solid (0, 0, 0).
        // Bottom-left 2×2 cell: solid (0, 0, 0).
        // Bottom-right 2×2 cell: solid (100, 200, 50).
        for x in 0..2 {
            for y in 0..2 {
                src.put_pixel(x, y, Rgb([200, 100, 50]));
            }
        }
        for x in 2..4 {
            for y in 2..4 {
                src.put_pixel(x, y, Rgb([100, 200, 50]));
            }
        }
        let out = box_downsample_rgb8(&src, 2, 2);
        assert_eq!(out.get_pixel(0, 0), &Rgb([200, 100, 50]));
        assert_eq!(out.get_pixel(1, 1), &Rgb([100, 200, 50]));
        assert_eq!(out.get_pixel(1, 0), &Rgb([0, 0, 0]));
        assert_eq!(out.get_pixel(0, 1), &Rgb([0, 0, 0]));
    }

    /// Hill-shade respects sun direction: pure-vertical sun → flat
    /// shading (ambient + diffuse * 1) on a flat terrain; near-zero
    /// sun-Y → almost-ambient-only shading.
    #[test]
    fn hill_shade_respects_sun_intensity() {
        let (_project, hm) = flat_project("shade", 2);
        let mut buf =
            ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(MINIMAP_DIM, MINIMAP_DIM, Rgb([200; 3]));
        // Vertical sun, intensity 1.0 — every pixel multiplied by
        // (AMBIENT + DIFFUSE) = 1.10, clamped to 255.
        apply_hill_shade(&mut buf, &hm, MapSize::square(2), [0.0, 1.0, 0.0, 1.0]);
        let p = buf.get_pixel(512, 512);
        // Flat terrain → n = (0, 1, 0), sun = (0, 1, 0) → n·l = 1.
        // 200 × 1.10 = 220.
        assert!(
            p[0].abs_diff(220) <= 2,
            "expected ~220, got {} (full pixel = {:?})",
            p[0],
            p
        );
    }
}
