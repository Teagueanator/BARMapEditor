//! DNTS (`splatDetailNormalTex`) bake pipeline — produces a BC3 / DXT5
//! DDS from a slot directory's `normal.png` (and optionally
//! `diffuse.{png,jpg}` when [`BakeOptions::diffuse_in_alpha`] is set).
//!
//! Per ADR-026.
//!
//! Pipeline:
//!   1. Read `normal.png` (PNG required — JPG normals destroy X/Y
//!      vector data per PITFALLS rule #2).
//!   2. Y-flip the normal's green channel iff `opts.yflip_normal`.
//!      Starter-pack default is `false` (D1 ships ambientCG
//!      `*_NormalGL.png`, already OpenGL convention per FINDINGS §7.4);
//!      `true` is for F23 user-imports of DirectX-source normals
//!      (Substance / Quixel exports).
//!   3. Compose RGBA8: RGB = (possibly Y-flipped) normal; A = 0xFF
//!      when `diffuse_in_alpha == false` (ADR-025 baseline), else
//!      diffuse luminance. The high-pass tuning (radius, sigma) is
//!      deferred to ADR-034 — this branch ships untested in BAR.
//!   4. Cache key = sha256(diffuse_bytes ++ normal_bytes ++ opts_bytes).
//!      If `tools/textures-cache/<sha>.dds` exists, copy it; else
//!      write the composed RGBA8 to a temp PNG, invoke
//!      `CompressonatorCLI -fd BC3 -nomipmap`, store the result in
//!      the cache, then copy to `out_dds`.
//!
//! Output is BC3 / DXT5 with alpha — that keeps the upgrade path to
//! the alpha-diffuse workflow (ADR-034) open without re-baking the
//! BCn format choice. The Compressonator subprocess pattern mirrors
//! [`crate::pymapconv`] (ADR-012): capture stdout + stderr, stream to
//! `tracing::trace!`, treat artifact-presence as the success contract.

use std::path::{Path, PathBuf};
use std::process::Command;

use image::{Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use tracing::{debug, info, trace, warn};

/// Per-slot bake configuration. Stable byte encoding via
/// [`BakeOptions::to_cache_bytes`] backs the sha256 cache key.
///
/// Defaults follow ADR-025: starter pack ships OpenGL-convention
/// normals (no Y-flip) and the alpha channel as solid 0xFF (the
/// `splatDetailNormalDiffuseAlpha = false` baseline). Both knobs are
/// exposed for F23 user-import flexibility and the future high-pass
/// workflow (ADR-034).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BakeOptions {
    /// Invert the green channel before compositing. `false` for the
    /// D1 starter pack (ambientCG `*_NormalGL.png` is already OpenGL
    /// convention). `true` for DirectX-source normals.
    pub yflip_normal: bool,
    /// When `true`, the diffuse luminance is written into the alpha
    /// channel. Plumbed but UNTESTED in BAR — high-pass tuning lives
    /// behind ADR-034. Ship `false` this sprint.
    pub diffuse_in_alpha: bool,
}

impl BakeOptions {
    /// Stable byte encoding for the cache key. Order is fixed.
    fn to_cache_bytes(self) -> [u8; 2] {
        [u8::from(self.yflip_normal), u8::from(self.diffuse_in_alpha)]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DntsBakeError {
    #[error("slot directory not found or not a directory: {0}")]
    SlotDirMissing(PathBuf),

    #[error("normal.png not found under {0}; run scripts/fetch-textures.sh from the repo root")]
    NormalMissing(PathBuf),

    #[error(
        "JPG normal at {0} is rejected — JPEG chroma subsampling destroys X/Y vector data (PITFALLS rule #2). Convert to PNG first."
    )]
    NormalNotPng(PathBuf),

    #[error(
        "diffuse_in_alpha=true requires a PNG diffuse but only {0} is present; convert to PNG or set diffuse_in_alpha=false"
    )]
    DiffuseNotPngForAlpha(PathBuf),

    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to decode normal image {path}: {source}")]
    DecodeNormal {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    #[error("failed to decode diffuse image {path}: {source}")]
    DecodeDiffuse {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    #[error(
        "normal and (when present) diffuse must share dimensions; got normal {n:?}, diffuse {d:?}"
    )]
    DimensionMismatch { n: (u32, u32), d: (u32, u32) },

    #[error(
        "CompressonatorCLI not found at {0}; run scripts/fetch-compressonator.sh from the repo root"
    )]
    CompressonatorMissing(PathBuf),

    #[error("failed to spawn CompressonatorCLI at {bin}: {source}")]
    SpawnCompressonator {
        bin: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "CompressonatorCLI failed (status {status:?}; input={input}; output={output})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        input = input.display(),
        output = output.display()
    )]
    CompressonatorFailed {
        status: Option<i32>,
        input: PathBuf,
        output: PathBuf,
        stdout: String,
        stderr: String,
    },

    #[error("CompressonatorCLI exited 0 but produced no DDS at {0}")]
    CompressonatorMissingOutput(PathBuf),
}

/// Bake a slot directory's `normal.png` (+ optional diffuse) to a
/// `splatDetailNormalTex`-format BC3 DDS at `out_dds`.
///
/// `slot_dir` is the per-slot directory laid out by ADR-027:
/// `<slot_dir>/normal.png` (required) and
/// `<slot_dir>/diffuse.{png,jpg}` (optional unless
/// `opts.diffuse_in_alpha` — then PNG required).
///
/// `out_dds` is the destination path. Parent directory must exist.
///
/// Cache lives under `<tools>/textures-cache/<sha>.dds`, where
/// `<tools>` is `slot_dir`'s grandparent (the on-disk layout of
/// ADR-014 / ADR-027 places `tools/textures/<slot>/` and
/// `tools/compressonator/` side by side). The cache key folds in
/// the raw bytes of both inputs plus [`BakeOptions::to_cache_bytes`],
/// so toggling `diffuse_in_alpha` or `yflip_normal` invalidates the
/// entry.
pub fn bake_dnts(slot_dir: &Path, out_dds: &Path, opts: BakeOptions) -> Result<(), DntsBakeError> {
    let env = BakeEnv::discover(slot_dir)?;
    bake_dnts_in_env(&env, slot_dir, out_dds, opts)
}

/// Internal entry point that takes an explicit environment.
/// Tests use this to redirect the cache + the Compressonator binary
/// path independent of the on-disk layout.
fn bake_dnts_in_env(
    env: &BakeEnv,
    slot_dir: &Path,
    out_dds: &Path,
    opts: BakeOptions,
) -> Result<(), DntsBakeError> {
    if !slot_dir.is_dir() {
        return Err(DntsBakeError::SlotDirMissing(slot_dir.to_path_buf()));
    }

    let normal_png = slot_dir.join("normal.png");
    let normal_jpg = slot_dir.join("normal.jpg");
    if !normal_png.is_file() {
        if normal_jpg.is_file() {
            return Err(DntsBakeError::NormalNotPng(normal_jpg));
        }
        return Err(DntsBakeError::NormalMissing(slot_dir.to_path_buf()));
    }

    let diffuse_png = slot_dir.join("diffuse.png");
    let diffuse_jpg = slot_dir.join("diffuse.jpg");
    let diffuse_path: Option<PathBuf> = if diffuse_png.is_file() {
        Some(diffuse_png)
    } else if diffuse_jpg.is_file() {
        Some(diffuse_jpg)
    } else {
        None
    };

    let normal_bytes = read_bytes(&normal_png)?;
    let diffuse_bytes = match diffuse_path.as_deref() {
        Some(p) => read_bytes(p)?,
        None => Vec::new(),
    };

    let key = cache_key(&diffuse_bytes, &normal_bytes, opts);
    let cache_path = env.cache_dir.join(format!("{key}.dds"));

    if cache_path.is_file() {
        info!(slot = %env.slot_name, cache = ?cache_path, "dnts: cache hit");
        ensure_parent_dir(out_dds)?;
        copy_file(&cache_path, out_dds)?;
        return Ok(());
    }

    info!(slot = %env.slot_name, cache_key = %key, "dnts: cache miss — baking");

    let normal_rgba = decode_rgba(&normal_png, false)?;
    let diffuse_rgba = if opts.diffuse_in_alpha {
        let p = diffuse_path
            .as_deref()
            .ok_or_else(|| DntsBakeError::NormalMissing(slot_dir.to_path_buf()))?;
        if p.extension().and_then(|s| s.to_str()) != Some("png") {
            return Err(DntsBakeError::DiffuseNotPngForAlpha(p.to_path_buf()));
        }
        Some(decode_rgba(p, true)?)
    } else {
        None
    };

    let mut composed = compose_dnts_rgba(&normal_rgba, diffuse_rgba.as_ref(), opts)?;
    if opts.yflip_normal {
        flip_green_channel(&mut composed);
    }

    let staging = env.cache_dir.join(format!("{key}.png"));
    ensure_parent_dir(&staging)?;
    composed
        .save(&staging)
        .map_err(|source| DntsBakeError::Io {
            path: staging.clone(),
            source: std::io::Error::other(source),
        })?;

    invoke_compressonator(
        &env.compressonator_bin,
        &env.compressonator_dir,
        &staging,
        &cache_path,
    )?;

    // Best-effort cleanup of the staging PNG — failure here doesn't break the
    // bake, just leaves a stray file in the cache dir.
    if let Err(e) = std::fs::remove_file(&staging) {
        warn!(?staging, %e, "dnts: could not remove staging png");
    }

    ensure_parent_dir(out_dds)?;
    copy_file(&cache_path, out_dds)?;
    Ok(())
}

/// Resolved environment for a bake invocation: where the
/// Compressonator binary lives and where the cache is.
#[derive(Debug)]
struct BakeEnv {
    /// Path to the underlying ELF (`compressonatorcli-bin`). The
    /// `CompressonatorCLI` symlink at the same dir is the
    /// fetch-script-ran canary; we invoke the ELF directly so we
    /// don't depend on the bash launcher's shebang resolution
    /// (Rust's exec path can ENOEXEC on the wrapper inside the
    /// test harness — see devlog).
    compressonator_bin: PathBuf,
    /// Directory the ELF lives in; needed for `LD_LIBRARY_PATH` so
    /// the bundled Qt + pkglibs `.so`s resolve. Mirrors the
    /// `compressonatorcli` wrapper script.
    compressonator_dir: PathBuf,
    cache_dir: PathBuf,
    slot_name: String,
}

impl BakeEnv {
    /// Discover the bake environment by walking up from `slot_dir`.
    /// Expects the ADR-014 / ADR-027 on-disk layout:
    /// `tools/textures/<slot>/` next to `tools/compressonator/`.
    fn discover(slot_dir: &Path) -> Result<Self, DntsBakeError> {
        let textures_dir = slot_dir
            .parent()
            .ok_or_else(|| DntsBakeError::SlotDirMissing(slot_dir.to_path_buf()))?;
        let tools_dir = textures_dir
            .parent()
            .ok_or_else(|| DntsBakeError::SlotDirMissing(slot_dir.to_path_buf()))?;

        let compressonator_dir = tools_dir.join("compressonator");
        let canary = compressonator_dir.join("CompressonatorCLI");
        if !canary.exists() {
            return Err(DntsBakeError::CompressonatorMissing(canary));
        }
        let bin = compressonator_dir.join("compressonatorcli-bin");
        if !bin.exists() {
            return Err(DntsBakeError::CompressonatorMissing(bin));
        }

        let cache_dir = tools_dir.join("textures-cache");
        std::fs::create_dir_all(&cache_dir).map_err(|source| DntsBakeError::Io {
            path: cache_dir.clone(),
            source,
        })?;

        let slot_name = slot_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();

        debug!(?bin, ?cache_dir, slot = %slot_name, "dnts: bake env");
        Ok(Self {
            compressonator_bin: bin,
            compressonator_dir,
            cache_dir,
            slot_name,
        })
    }
}

/// Compose the final RGBA8 image fed to Compressonator. Pure function;
/// the optional Y-flip step is applied separately by the caller so
/// tests can target either step in isolation.
fn compose_dnts_rgba(
    normal: &RgbaImage,
    diffuse: Option<&RgbaImage>,
    opts: BakeOptions,
) -> Result<RgbaImage, DntsBakeError> {
    let (w, h) = normal.dimensions();
    if let Some(d) = diffuse
        && d.dimensions() != (w, h)
    {
        return Err(DntsBakeError::DimensionMismatch {
            n: (w, h),
            d: d.dimensions(),
        });
    }

    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let np = normal.get_pixel(x, y).0;
            let alpha = if opts.diffuse_in_alpha {
                let d = diffuse
                    .expect("diffuse present per dim check")
                    .get_pixel(x, y)
                    .0;
                rec709_luma(d[0], d[1], d[2])
            } else {
                0xFF
            };
            out.put_pixel(x, y, Rgba([np[0], np[1], np[2], alpha]));
        }
    }
    Ok(out)
}

/// Rec.709 luma → integer 0..=255. Used as the placeholder
/// `diffuse_in_alpha` payload; ADR-034 swaps this for a high-pass
/// filter once the in-engine A/B confirms the math.
fn rec709_luma(r: u8, g: u8, b: u8) -> u8 {
    let y = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    y.round().clamp(0.0, 255.0) as u8
}

/// In-place invert of the green channel — `g ← 255 - g`. Per FINDINGS
/// §7.4 the engine builds the TBN with +Y up and decodes splat normals
/// as `* 2 - 1`; sources authored under DirectX convention need this
/// step or every DNTS layer's lighting is upside-down on slopes.
fn flip_green_channel(img: &mut RgbaImage) {
    for px in img.pixels_mut() {
        px.0[1] = 255 - px.0[1];
    }
}

/// Sha256 over (diffuse_bytes ‖ normal_bytes ‖ opts_bytes), hex-encoded.
fn cache_key(diffuse_bytes: &[u8], normal_bytes: &[u8], opts: BakeOptions) -> String {
    let mut h = Sha256::new();
    h.update(diffuse_bytes);
    h.update(normal_bytes);
    h.update(opts.to_cache_bytes());
    let digest = h.finalize();
    hex_lower(&digest)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xF) as usize] as char);
    }
    out
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, DntsBakeError> {
    std::fs::read(path).map_err(|source| DntsBakeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn decode_rgba(path: &Path, diffuse: bool) -> Result<RgbaImage, DntsBakeError> {
    let img = image::open(path).map_err(|source| {
        if diffuse {
            DntsBakeError::DecodeDiffuse {
                path: path.to_path_buf(),
                source,
            }
        } else {
            DntsBakeError::DecodeNormal {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    Ok(img.to_rgba8())
}

fn ensure_parent_dir(path: &Path) -> Result<(), DntsBakeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| DntsBakeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), DntsBakeError> {
    std::fs::copy(src, dst).map_err(|source| DntsBakeError::Io {
        path: dst.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn invoke_compressonator(
    bin: &Path,
    bin_dir: &Path,
    in_png: &Path,
    out_dds: &Path,
) -> Result<(), DntsBakeError> {
    // Mirror the `compressonatorcli` wrapper script: prepend the
    // compressonator dir + its `qt/` and `pkglibs/` subdirs to
    // LD_LIBRARY_PATH so the bundled Qt and pkg `.so`s resolve.
    let mut ld_entries: Vec<PathBuf> = vec![
        bin_dir.to_path_buf(),
        bin_dir.join("qt"),
        bin_dir.join("pkglibs"),
    ];
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        ld_entries.extend(std::env::split_paths(&existing));
    }
    let ld_path = std::env::join_paths(ld_entries).expect("vendored ld paths");

    let mut cmd = Command::new(bin);
    cmd.env("LD_LIBRARY_PATH", &ld_path)
        .arg("-fd")
        .arg("BC3")
        .arg("-nomipmap")
        .arg(in_png)
        .arg(out_dds);

    debug!(?cmd, "dnts: invoking CompressonatorCLI");
    let output = cmd
        .output()
        .map_err(|source| DntsBakeError::SpawnCompressonator {
            bin: bin.to_path_buf(),
            source,
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // Compressonator emits useful warnings on stderr (e.g. NPOT
    // dimensions). Stream both at trace so they're available under
    // `RUST_LOG=barme_pipeline::dnts=trace` but don't pollute info-level.
    if !stdout.is_empty() {
        trace!(target: "barme_pipeline::dnts", "compressonator stdout:\n{stdout}");
    }
    if !stderr.is_empty() {
        trace!(target: "barme_pipeline::dnts", "compressonator stderr:\n{stderr}");
    }

    // Trust artifact presence as the success contract (mirrors the
    // PyMapConv driver). Compressonator generally exits 0 on success,
    // but a non-zero exit with the DDS present and well-formed has
    // been seen in the field; warn-and-accept rather than fail.
    let present = out_dds.is_file();
    if !output.status.success() && !present {
        return Err(DntsBakeError::CompressonatorFailed {
            status: output.status.code(),
            input: in_png.to_path_buf(),
            output: out_dds.to_path_buf(),
            stdout,
            stderr,
        });
    }
    if !present {
        return Err(DntsBakeError::CompressonatorMissingOutput(
            out_dds.to_path_buf(),
        ));
    }
    if !output.status.success() {
        warn!(
            status = ?output.status,
            ?out_dds,
            "dnts: compressonator non-zero exit but DDS produced; accepting"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use std::fs;

    fn synth_normal(w: u32, h: u32, fill: [u8; 4]) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for px in img.pixels_mut() {
            *px = Rgba(fill);
        }
        img
    }

    #[test]
    fn flip_green_inverts_and_preserves_other_channels() {
        let mut img = synth_normal(4, 4, [0x10, 0x40, 0xC0, 0xFF]);
        flip_green_channel(&mut img);
        for px in img.pixels() {
            assert_eq!(px.0[0], 0x10, "R untouched");
            assert_eq!(px.0[1], 255 - 0x40, "G inverted");
            assert_eq!(px.0[2], 0xC0, "B untouched");
            assert_eq!(px.0[3], 0xFF, "A untouched");
        }
    }

    #[test]
    fn flip_green_at_boundaries() {
        // 0 → 255, 255 → 0, 128 → 127, 127 → 128.
        let mut img = RgbaImage::new(4, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0xFF]));
        img.put_pixel(1, 0, Rgba([0, 255, 0, 0xFF]));
        img.put_pixel(2, 0, Rgba([0, 128, 0, 0xFF]));
        img.put_pixel(3, 0, Rgba([0, 127, 0, 0xFF]));
        flip_green_channel(&mut img);
        assert_eq!(img.get_pixel(0, 0).0[1], 255);
        assert_eq!(img.get_pixel(1, 0).0[1], 0);
        assert_eq!(img.get_pixel(2, 0).0[1], 127);
        assert_eq!(img.get_pixel(3, 0).0[1], 128);
    }

    #[test]
    fn passthrough_is_identity_when_flag_off() {
        let original = synth_normal(8, 8, [0xAA, 0x55, 0x33, 0xFF]);
        let mut clone = original.clone();
        // Mirror the bake's caller-side guard: only flip when the
        // flag is true. Off-branch is a no-op.
        let opts = BakeOptions {
            yflip_normal: false,
            diffuse_in_alpha: false,
        };
        if opts.yflip_normal {
            flip_green_channel(&mut clone);
        }
        assert_eq!(clone, original);
    }

    #[test]
    fn compose_alpha_solid_when_diffuse_in_alpha_false() {
        let normal = synth_normal(2, 2, [128, 128, 255, 0]);
        let opts = BakeOptions::default();
        let out = compose_dnts_rgba(&normal, None, opts).unwrap();
        for px in out.pixels() {
            assert_eq!(px.0[3], 0xFF, "alpha solid under default opts");
            assert_eq!(px.0[0], 128);
            assert_eq!(px.0[1], 128);
            assert_eq!(px.0[2], 255);
        }
    }

    #[test]
    fn compose_alpha_is_luminance_when_diffuse_in_alpha_true() {
        let normal = synth_normal(2, 2, [128, 128, 255, 0]);
        // Diffuse = pure red. Rec.709 Y = 0.2126*255 ≈ 54.
        let diffuse = synth_normal(2, 2, [255, 0, 0, 0xFF]);
        let opts = BakeOptions {
            yflip_normal: false,
            diffuse_in_alpha: true,
        };
        let out = compose_dnts_rgba(&normal, Some(&diffuse), opts).unwrap();
        for px in out.pixels() {
            assert_eq!(px.0[3], 54, "rec.709 luma of pure red");
        }
    }

    #[test]
    fn cache_key_is_stable_and_sensitive_to_opts() {
        let dif = b"diffuse-bytes".to_vec();
        let norm = b"normal-bytes".to_vec();
        let a = cache_key(&dif, &norm, BakeOptions::default());
        let b = cache_key(&dif, &norm, BakeOptions::default());
        assert_eq!(a, b, "deterministic on identical inputs");

        let opts_flip = BakeOptions {
            yflip_normal: true,
            diffuse_in_alpha: false,
        };
        assert_ne!(a, cache_key(&dif, &norm, opts_flip));

        let opts_alpha = BakeOptions {
            yflip_normal: false,
            diffuse_in_alpha: true,
        };
        assert_ne!(a, cache_key(&dif, &norm, opts_alpha));

        // Different bytes → different key.
        let mut alt = norm.clone();
        alt[0] ^= 0xFF;
        assert_ne!(a, cache_key(&dif, &alt, BakeOptions::default()));
    }

    fn locate_compressonator() -> Option<(PathBuf, PathBuf)> {
        // CARGO_MANIFEST_DIR points at crates/barme-pipeline; the
        // vendored binary lives at
        // <repo>/tools/compressonator/compressonatorcli-bin.
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let dir = manifest
            .parent()? // crates/
            .parent()? // repo root
            .join("tools")
            .join("compressonator");
        let bin = dir.join("compressonatorcli-bin");
        bin.exists().then_some((bin, dir))
    }

    /// Build a synthetic slot directory inside `root`.
    fn build_synth_slot(root: &Path, name: &str, fill: [u8; 4]) -> PathBuf {
        let slot = root.join(name);
        fs::create_dir_all(&slot).unwrap();
        synth_normal(8, 8, fill)
            .save(slot.join("normal.png"))
            .unwrap();
        // Diffuse PNG so JPG-detection guard is exercised in the negative path.
        synth_normal(8, 8, [200, 50, 25, 255])
            .save(slot.join("diffuse.png"))
            .unwrap();
        slot
    }

    #[test]
    fn rejects_jpg_normal() {
        let tmp = tempfile::tempdir().unwrap();
        let slot = tmp.path().join("99-test-slot");
        fs::create_dir_all(&slot).unwrap();
        // Drop a placeholder .jpg (contents don't matter — we check
        // extension before decoding).
        fs::write(slot.join("normal.jpg"), b"not a real jpeg").unwrap();
        let env = BakeEnv {
            compressonator_bin: PathBuf::from("/nonexistent"),
            compressonator_dir: PathBuf::from("/nonexistent"),
            cache_dir: tmp.path().join("cache"),
            slot_name: "99-test-slot".into(),
        };
        let err = bake_dnts_in_env(
            &env,
            &slot,
            &tmp.path().join("out.dds"),
            BakeOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, DntsBakeError::NormalNotPng(_)), "got {err:?}");
    }

    #[test]
    fn missing_normal_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let slot = tmp.path().join("99-empty-slot");
        fs::create_dir_all(&slot).unwrap();
        let env = BakeEnv {
            compressonator_bin: PathBuf::from("/nonexistent"),
            compressonator_dir: PathBuf::from("/nonexistent"),
            cache_dir: tmp.path().join("cache"),
            slot_name: "99-empty-slot".into(),
        };
        let err = bake_dnts_in_env(
            &env,
            &slot,
            &tmp.path().join("out.dds"),
            BakeOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, DntsBakeError::NormalMissing(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn end_to_end_bake_and_cache_round_trip() {
        let Some((bin, dir)) = locate_compressonator() else {
            eprintln!("skipping: compressonatorcli-bin not vendored");
            return;
        };

        let tmp = tempfile::tempdir().unwrap();
        let slot = build_synth_slot(tmp.path(), "00-test-slot", [128, 128, 255, 0xFF]);
        let cache_dir = tmp.path().join("textures-cache");
        let env = BakeEnv {
            compressonator_bin: bin.clone(),
            compressonator_dir: dir.clone(),
            cache_dir: cache_dir.clone(),
            slot_name: "00-test-slot".into(),
        };
        let out_dds = tmp.path().join("out.dds");

        // First bake: cache miss → real Compressonator invocation.
        bake_dnts_in_env(&env, &slot, &out_dds, BakeOptions::default()).unwrap();
        assert!(out_dds.is_file());
        let first_mtime = out_dds.metadata().unwrap().modified().unwrap();
        let cached: Vec<_> = fs::read_dir(&cache_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        let dds_count = cached
            .iter()
            .filter(|n| n.to_string_lossy().ends_with(".dds"))
            .count();
        assert_eq!(dds_count, 1, "exactly one cached DDS after first bake");

        // DDS magic: little-endian "DDS " at offset 0.
        let dds_bytes = fs::read(&out_dds).unwrap();
        assert!(
            dds_bytes.starts_with(b"DDS "),
            "expected DDS magic, got {:?}",
            &dds_bytes[..4.min(dds_bytes.len())]
        );

        // Second bake: identical inputs → cache hit, no new DDS in cache.
        std::thread::sleep(std::time::Duration::from_millis(10));
        bake_dnts_in_env(&env, &slot, &out_dds, BakeOptions::default()).unwrap();
        let dds_count2 = fs::read_dir(&cache_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".dds"))
            .count();
        assert_eq!(dds_count2, 1, "cache hit produces no new entries");
        let second_mtime = out_dds.metadata().unwrap().modified().unwrap();
        assert!(
            second_mtime >= first_mtime,
            "out_dds copied on cache hit (mtime monotonic)"
        );

        // Toggle diffuse_in_alpha → cache miss → a second DDS lands in cache.
        let opts_alpha = BakeOptions {
            yflip_normal: false,
            diffuse_in_alpha: true,
        };
        bake_dnts_in_env(&env, &slot, &out_dds, opts_alpha).unwrap();
        let dds_count3 = fs::read_dir(&cache_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".dds"))
            .count();
        assert_eq!(dds_count3, 2, "diffuse_in_alpha toggle invalidates cache");
    }

    #[test]
    fn bake_options_default_matches_adr_025_baseline() {
        let opts = BakeOptions::default();
        assert!(!opts.yflip_normal, "ambientCG _NormalGL is already OpenGL");
        assert!(
            !opts.diffuse_in_alpha,
            "splatDetailNormalDiffuseAlpha = false is the safer ship default"
        );
        assert_eq!(opts.to_cache_bytes(), [0, 0]);
    }

    #[test]
    fn cache_bytes_encode_each_flag_in_a_distinct_position() {
        // Cache key sensitivity smoke: the two flags must be
        // distinguishable in the encoded bytes so the sha256 picks up
        // each independently.
        let only_yflip = BakeOptions {
            yflip_normal: true,
            diffuse_in_alpha: false,
        };
        let only_alpha = BakeOptions {
            yflip_normal: false,
            diffuse_in_alpha: true,
        };
        let both = BakeOptions {
            yflip_normal: true,
            diffuse_in_alpha: true,
        };
        assert_eq!(only_yflip.to_cache_bytes(), [1, 0]);
        assert_eq!(only_alpha.to_cache_bytes(), [0, 1]);
        assert_eq!(both.to_cache_bytes(), [1, 1]);
    }

    #[test]
    fn compose_dimension_mismatch_errors() {
        let normal = synth_normal(4, 4, [0, 0, 255, 0]);
        let diffuse = synth_normal(8, 4, [255, 255, 255, 255]);
        let opts = BakeOptions {
            yflip_normal: false,
            diffuse_in_alpha: true,
        };
        let err = compose_dnts_rgba(&normal, Some(&diffuse), opts).unwrap_err();
        assert!(
            matches!(err, DntsBakeError::DimensionMismatch { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn diffuse_in_alpha_with_only_jpg_diffuse_errors() {
        // bake_dnts_in_env with `diffuse_in_alpha: true` and a JPG
        // diffuse-only present must surface DiffuseNotPngForAlpha.
        let tmp = tempfile::tempdir().unwrap();
        let slot = tmp.path().join("99-diffuse-jpg-slot");
        fs::create_dir_all(&slot).unwrap();
        synth_normal(4, 4, [128, 128, 255, 0xFF])
            .save(slot.join("normal.png"))
            .unwrap();
        fs::write(slot.join("diffuse.jpg"), b"not-a-real-jpeg").unwrap();
        let env = BakeEnv {
            compressonator_bin: PathBuf::from("/nonexistent"),
            compressonator_dir: PathBuf::from("/nonexistent"),
            cache_dir: tmp.path().join("cache"),
            slot_name: "99-diffuse-jpg-slot".into(),
        };
        let opts = BakeOptions {
            yflip_normal: false,
            diffuse_in_alpha: true,
        };
        let err = bake_dnts_in_env(&env, &slot, &tmp.path().join("out.dds"), opts).unwrap_err();
        assert!(
            matches!(err, DntsBakeError::DiffuseNotPngForAlpha(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn slot_dir_missing_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let env = BakeEnv {
            compressonator_bin: PathBuf::from("/nonexistent"),
            compressonator_dir: PathBuf::from("/nonexistent"),
            cache_dir: tmp.path().join("cache"),
            slot_name: "<missing>".into(),
        };
        let err = bake_dnts_in_env(
            &env,
            &tmp.path().join("does-not-exist"),
            &tmp.path().join("out.dds"),
            BakeOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(err, DntsBakeError::SlotDirMissing(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn discover_errors_when_compressonator_missing() {
        // `discover` walks two parents from slot_dir to find tools/;
        // if tools/compressonator/CompressonatorCLI isn't there, error
        // out before any bake work.
        let tmp = tempfile::tempdir().unwrap();
        let tools = tmp.path().join("tools");
        let slot = tools.join("textures").join("00-test-slot");
        fs::create_dir_all(&slot).unwrap();
        let err = BakeEnv::discover(&slot).unwrap_err();
        assert!(
            matches!(err, DntsBakeError::CompressonatorMissing(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn cache_hit_produces_identical_bytes() {
        // Confirms the cache-copy path is byte-faithful — not just
        // "any DDS appears" but "the same DDS as last time."
        let Some((bin, dir)) = locate_compressonator() else {
            eprintln!("skipping: compressonatorcli-bin not vendored");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let slot = build_synth_slot(tmp.path(), "00-test-slot", [128, 128, 255, 0xFF]);
        let env = BakeEnv {
            compressonator_bin: bin,
            compressonator_dir: dir,
            cache_dir: tmp.path().join("textures-cache"),
            slot_name: "00-test-slot".into(),
        };
        let out_a = tmp.path().join("a.dds");
        let out_b = tmp.path().join("b.dds");
        bake_dnts_in_env(&env, &slot, &out_a, BakeOptions::default()).unwrap();
        bake_dnts_in_env(&env, &slot, &out_b, BakeOptions::default()).unwrap();
        let bytes_a = fs::read(&out_a).unwrap();
        let bytes_b = fs::read(&out_b).unwrap();
        assert_eq!(bytes_a, bytes_b, "cache hit must produce identical bytes");
        assert!(bytes_a.starts_with(b"DDS "), "DDS magic preserved");
    }

    #[test]
    fn yflip_through_compose_inverts_g_only() {
        // End-to-end check: compose_dnts_rgba → flip_green_channel
        // mirrors the order the bake function uses (compose first,
        // then flip the composed RGBA). Verifies the G of the
        // *normal* gets flipped, not the alpha derived from diffuse.
        let normal = synth_normal(2, 2, [10, 200, 50, 0]);
        let diffuse = synth_normal(2, 2, [255, 255, 255, 0xFF]);
        let opts = BakeOptions {
            yflip_normal: true,
            diffuse_in_alpha: true,
        };
        let mut composed = compose_dnts_rgba(&normal, Some(&diffuse), opts).unwrap();
        flip_green_channel(&mut composed);
        for px in composed.pixels() {
            assert_eq!(px.0[0], 10, "R unchanged through compose+flip");
            assert_eq!(px.0[1], 255 - 200, "G inverted by Y-flip step");
            assert_eq!(px.0[2], 50, "B unchanged");
            assert_eq!(px.0[3], 255, "A from luma(255,255,255) == 255");
        }
    }

    #[test]
    fn cache_key_sha256_is_deterministic_hex_64() {
        // The cache filename derives from this; assert it's a stable
        // 64-char lowercase hex string. Drift in the digest format
        // would orphan every cache entry on the host.
        let key = cache_key(&[], &[], BakeOptions::default());
        assert_eq!(key.len(), 64);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "lowercase hex only: {key}"
        );
    }
}
