//! Generate test fixtures into `assets/fixtures/`.
//!
//! Run from the repo root:
//!     cargo run -p barme-core --example gen-fixture
//!
//! See ADR-007 — fixtures are generated, not committed.

use std::path::PathBuf;

use anyhow::Result;
use barme_core::{Heightmap, MapSize};

fn main() -> Result<()> {
    // CARGO_MANIFEST_DIR points at crates/barme-core; the repo root is two up.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().and_then(|p| p.parent()).unwrap();
    let out_dir = repo_root.join("assets").join("fixtures");
    std::fs::create_dir_all(&out_dir)?;

    for smu in [2u32, 4, 16] {
        let size = MapSize::square(smu);
        let h = Heightmap::synth_ramp(size);
        let (w, _) = h.dims();
        let path = out_dir.join(format!("r16_ramp_{smu}x{smu}smu_{w}px.png"));
        h.save_png(&path)?;
        println!(
            "wrote {} ({}×{}, {} samples)",
            path.display(),
            w,
            w,
            h.data().len()
        );
    }

    Ok(())
}
