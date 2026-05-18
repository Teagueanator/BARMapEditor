//! One-shot: build a tiny smoke map and write it straight into BAR's user
//! maps directory. Stage 0 hand-off to goal #7 (in-engine validation).
//!
//! Usage:
//!     cargo run -p barme-pipeline --example build_smoke
//!
//! Hardcoded to a 2-SMU stub named `teague-test-1` so the artefact is easy
//! to spot in BAR's map browser. This will be replaced by a real launcher
//! (planned ADR-015) — this example is the seed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use barme_core::{Heightmap, MapSize, Project};
use barme_pipeline::{PyMapConvDriver, build_sd7};
use image::{ImageBuffer, Rgb};
use tracing_subscriber::EnvFilter;

const MAP_NAME: &str = "teague-test-1";
const BAR_MAPS_DIR: &str = "/home/teague/.local/state/Beyond All Reason/maps";

/// Map dimensions in Spring Map Units. 8 SMU = 4096 elmos = the smallest
/// "small duel" size BAR maps typically use. (2 SMU is the PyMapConv floor
/// and looks like a postage stamp in-engine — see Stage 0 session log.)
const MAP_SMU: u32 = 8;

/// Heightmap altitude ceiling in elmos. 512 gives visible relief on the
/// synth-ramp gradient at 8 SMU without dominating the map.
const MAP_MAX_HEIGHT: f32 = 512.0;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let repo_root = repo_root();
    let driver = PyMapConvDriver::vendored(&repo_root)
        .context("vendored pymapconv + compressonator must be present (see ADR-011/ADR-014)")?;

    let workdir = tempfile::tempdir().context("create tempdir")?;
    let work = workdir.path();

    let mut project = Project::new(MAP_NAME, MAP_SMU);
    project.max_height = MAP_MAX_HEIGHT;

    let hm_png = work.join("heightmap.png");
    Heightmap::synth_ramp(MapSize::square(MAP_SMU))
        .save_png(&hm_png)
        .context("write heightmap PNG")?;

    // Texture dims in BAR are 512 px per SMU; PyMapConv requires multiples
    // of 1024 (= 2 SMU), which 8 SMU satisfies trivially (4096×4096 BMP,
    // ~48 MB in RAM during the synth + handoff).
    let tex_bmp = work.join("texture.bmp");
    let (tw, th) = project.size.texture_dims();
    write_grey_bmp(&tex_bmp, tw, th, [128, 128, 128]);

    let bar_dir = Path::new(BAR_MAPS_DIR);
    if !bar_dir.is_dir() {
        anyhow::bail!(
            "BAR maps dir does not exist at {}; install BAR or update BAR_MAPS_DIR in this example",
            bar_dir.display()
        );
    }
    let out_sd7 = bar_dir.join(format!("{MAP_NAME}.sd7"));

    let sd7 = build_sd7(&driver, &project, &hm_png, &tex_bmp, work, &out_sd7)
        .context("build_sd7 end-to-end")?;

    let bytes = std::fs::metadata(&sd7)?.len();
    println!("\nwrote {} ({bytes} bytes)", sd7.display());
    println!("launch BAR; look for `{MAP_NAME}` in the map browser.");
    Ok(())
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two parents up from a member crate")
        .to_path_buf()
}

fn write_grey_bmp(path: &Path, w: u32, h: u32, [r, g, b]: [u8; 3]) {
    let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(w, h, Rgb([r, g, b]));
    buf.save(path).expect("write BMP");
}
