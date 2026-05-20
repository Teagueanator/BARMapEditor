//! D8 / Sprint 15 (ADR-038) smoke binary: build a tiny 2-layer
//! [`LayerStack`], run [`LayerStack::bake_diffuse`], and write the
//! result to `/tmp/bake_layered_smoke.bmp` for visual inspection.
//!
//! Usage:
//!     cargo run -p barme-app --example bake_layered_smoke
//!
//! The output BMP at `/tmp/bake_layered_smoke.bmp` should show
//! slot 0's `diffuse.png` (grass meadow) wallpaper-tiled, with the
//! left half of the canvas tinted by a second layer carrying slot 4's
//! diffuse at 50 % opacity — a quick "did the alpha-over composite
//! land correctly?" eyeball test.
//!
//! No `.sd7` is produced; the bake target is the BMP only. The
//! `barme-pipeline::examples::build_smoke` example covers the
//! `.sd7` round-trip with the layered diffuse plumbed in.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use barme_core::{ClosureSlotResolver, LayerMask, LayerSource, LayerStack, MapSize, TextureLayer};
use tracing_subscriber::EnvFilter;

const OUT_PATH: &str = "/tmp/bake_layered_smoke.bmp";
const MAP_SMU: u32 = 4;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let repo = repo_root();
    let textures = repo.join("tools/textures");
    if !textures.is_dir() {
        anyhow::bail!(
            "starter texture pack not found at {} — run scripts/fetch-textures.sh first",
            textures.display()
        );
    }
    let resolver = ClosureSlotResolver(move |id: u8| match id {
        0 => Some(textures.join("00-grass-meadow/diffuse.png")),
        4 => Some(textures.join("04-desert-sand-dunes/diffuse.png")),
        // Fall back to slot 0 for anything else — keeps the bake
        // visible even on partially-populated registries.
        _ => Some(textures.join("00-grass-meadow/diffuse.png")),
    });

    let size = MapSize::square(MAP_SMU);
    let (tw, th) = size.texture_dims();
    println!("baking {MAP_SMU}-SMU map: diffuse dims = {tw}×{th}, output = {OUT_PATH}");

    // Layer 0: grass-meadow base, full mask, identity transform.
    let mut base = TextureLayer::new(LayerSource::Slot { id: 0 }, size, 255);
    base.name = "Base — grass".to_string();

    // Layer 1: desert-sand on the LEFT HALF, 50% opacity. Mask is
    // 255 on the left half (x < tw/2), 0 on the right half. The
    // 50% opacity multiplier means the right edge of the mask
    // fades 50% into the base rather than the texture replacing
    // it wholesale.
    let mut sand = TextureLayer::new(LayerSource::Slot { id: 4 }, size, 0);
    sand.name = "Top — desert (50%)".to_string();
    sand.opacity = 0.5;
    paint_left_half(&mut sand.mask);

    let stack = LayerStack {
        layers: vec![base, sand],
    };
    println!(
        "stack: {} layers, resident masks = {} bytes",
        stack.layers.len(),
        stack.resident_mask_bytes(),
    );

    let baked = stack.bake_diffuse(size, &resolver);
    baked
        .save(Path::new(OUT_PATH))
        .with_context(|| format!("write {OUT_PATH}"))?;
    let bytes = std::fs::metadata(OUT_PATH)?.len();
    println!("wrote {OUT_PATH} ({bytes} bytes)");
    println!(
        "open in an image viewer — left half should show desert-tinted grass, \
         right half should show pure grass."
    );
    Ok(())
}

fn paint_left_half(mask: &mut LayerMask) {
    // Sprint 16 (ADR-039): mask storage is tiled COW; the functional
    // `write_rect_with` writes the left half in one call without
    // exposing the underlying tile layout.
    let half_w = mask.width / 2;
    let h = mask.height;
    mask.write_rect_with(0, 0, half_w, h, |_, _| 255);
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two parents up from the example crate")
        .to_path_buf()
}
