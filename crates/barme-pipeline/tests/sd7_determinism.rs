//! End-to-end `.sd7` build determinism (Sprint 33 / T6 / ADR-049).
//!
//! NFR-Determinism: the same project must compile to a byte-identical
//! `.sd7`. The fast, CI-gated half of this guarantee lives in
//! `sd7::tests::package_is_byte_identical_on_repeat` (packaging layer,
//! needs only system 7z). THIS test exercises the *whole* pipeline —
//! PyMapConv SMF/SMT compile + mapinfo emit + packaging — and so needs
//! the vendored toolchain. Same `#[ignore]` policy as `build_sd7.rs`:
//!
//!     cargo test -p barme-pipeline --test sd7_determinism -- --ignored --nocapture
//!
//! PITFALL #3 / #10: if this ever flakes, the non-determinism is almost
//! certainly PyMapConv-side (SMT/Compressonator). The packaging-layer
//! test isolates *our* contribution; a flake here with that test green
//! means the issue is upstream — record a flake-rate in the devlog
//! rather than chasing our own code.

use std::path::PathBuf;

use barme_core::{Heightmap, MapSize, Project};
use barme_pipeline::{PyMapConvDriver, build_sd7};
use image::{ImageBuffer, Rgb};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two parents up from a member crate")
        .to_path_buf()
}

/// Build the minimal fixture project to a fresh `.sd7` under `work`.
fn build_minimal(driver: &PyMapConvDriver, work: &std::path::Path, tag: &str) -> PathBuf {
    let project = Project::new("determinism", 2);

    let hm_png = work.join(format!("heightmap_{tag}.png"));
    Heightmap::synth_ramp(MapSize::square(2))
        .save_png(&hm_png)
        .expect("write 16-bit PNG");

    let tex_bmp = work.join(format!("texture_{tag}.bmp"));
    let buf: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(1024, 1024, Rgb([128, 128, 128]));
    buf.save(&tex_bmp).expect("write BMP");

    let out_sd7 = work.join(format!("determinism_{tag}.sd7"));
    build_sd7(
        driver,
        &project,
        &hm_png,
        &tex_bmp,
        barme_pipeline::SplatBakeInputs::default(),
        None,
        None,
        work,
        &out_sd7,
    )
    .unwrap_or_else(|e| panic!("build {tag} failed:\n{e}"))
}

#[test]
#[ignore = "needs vendored pymapconv + compressonator + system 7z"]
fn sd7_byte_identical_on_repeat() {
    let root = repo_root();
    let driver = PyMapConvDriver::vendored(&root)
        .expect("vendored toolchain must be present; see ADR-011 / ADR-014");

    let workdir = tempfile::tempdir().expect("create tempdir");
    let work = workdir.path();

    let sd7_a = build_minimal(&driver, work, "a");
    let sd7_b = build_minimal(&driver, work, "b");

    let bytes_a = std::fs::read(&sd7_a).unwrap();
    let bytes_b = std::fs::read(&sd7_b).unwrap();
    assert_eq!(
        bytes_a.len(),
        bytes_b.len(),
        "build is non-deterministic (size differs: {} vs {} bytes) — \
         if the packaging-layer test is green, suspect PyMapConv/SMT",
        bytes_a.len(),
        bytes_b.len()
    );
    assert_eq!(
        bytes_a, bytes_b,
        "build is non-deterministic (bytes differ at equal length) — \
         NFR-Determinism violated"
    );
}
