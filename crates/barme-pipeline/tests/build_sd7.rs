//! End-to-end `.sd7` build test (ADR-013).
//!
//! Same `#[ignore]` policy as `compile_smf.rs`: needs the vendored PyMapConv,
//! vendored Compressonator (ADR-014), and a system 7-Zip binary. Run with:
//!
//!     cargo test -p barme-pipeline -- --ignored --nocapture

use std::path::PathBuf;
use std::process::Command;

use barme_core::{Heightmap, MapSize, Project};
use barme_pipeline::{PyMapConvDriver, build_sd7};
use image::{ImageBuffer, Rgb};
use tracing_subscriber::EnvFilter;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two parents up from a member crate")
        .to_path_buf()
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_test_writer()
        .try_init();
}

fn write_grey_bmp(path: &std::path::Path, w: u32, h: u32, [r, g, b]: [u8; 3]) {
    let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(w, h, Rgb([r, g, b]));
    buf.save(path).expect("write BMP");
}

#[test]
#[ignore = "needs vendored pymapconv + compressonator + system 7z"]
fn build_sd7_end_to_end() {
    init_tracing();

    let root = repo_root();
    let driver = PyMapConvDriver::vendored(&root)
        .expect("vendored toolchain must be present; see ADR-011 / ADR-014");

    let workdir = tempfile::tempdir().expect("create tempdir");
    let work = workdir.path();

    let project = Project::new("smoke", 2);

    let hm_png = work.join("heightmap.png");
    Heightmap::synth_ramp(MapSize::square(2))
        .save_png(&hm_png)
        .expect("write 16-bit PNG");

    let tex_bmp = work.join("texture.bmp");
    write_grey_bmp(&tex_bmp, 1024, 1024, [128, 128, 128]);

    let out_sd7 = work.join("smoke.sd7");

    let result = build_sd7(
        &driver,
        &project,
        &hm_png,
        &tex_bmp,
        barme_pipeline::SplatBakeInputs::default(),
        work,
        &out_sd7,
    )
    .unwrap_or_else(|e| panic!("build failed:\n{e}"));

    assert_eq!(result, out_sd7);
    assert!(out_sd7.is_file(), "sd7 missing at {}", out_sd7.display());
    let bytes = std::fs::metadata(&out_sd7).unwrap().len();
    assert!(bytes > 0, "sd7 is empty");
    eprintln!("sd7: {} ({} bytes)", out_sd7.display(), bytes);

    // Independent verification: inspect via 7-Zip directly. We don't trust
    // our own `verify_non_solid` here — that's the production-side check;
    // this is a parallel pair-of-eyes assertion against the same file.
    let seven = which::which("7zz")
        .or_else(|_| which::which("7z"))
        .or_else(|_| which::which("7za"))
        .expect("a system 7-Zip binary");
    let listing = Command::new(&seven)
        .arg("l")
        .arg("-slt")
        .arg(&out_sd7)
        .output()
        .expect("7z l -slt");
    let listing_str = String::from_utf8_lossy(&listing.stdout);
    eprintln!("--- 7z listing ---\n{listing_str}");
    assert!(listing.status.success(), "7z l -slt failed");
    assert!(
        listing_str.lines().any(|l| l.trim() == "Solid = -"),
        "archive came out solid — PITFALL #9!"
    );

    // The archive's required entries: maps/<name>.smf, maps/<name>.smt,
    // mapinfo.lua. Each must appear as its own `Path = ...` line.
    for needle in &[
        "Path = maps/smoke.smf",
        "Path = maps/smoke.smt",
        "Path = mapinfo.lua",
    ] {
        assert!(
            listing_str.contains(needle),
            "{} not found in archive listing",
            needle
        );
    }
}
