//! End-to-end PyMapConv compile test.
//!
//! Marked `#[ignore]` so `cargo test --workspace` stays hermetic — running it
//! requires the vendored PyMapConv binary on disk
//! (`scripts/fetch-pymapconv.sh`). Invoke explicitly:
//!
//!     cargo test -p barme-pipeline -- --ignored --nocapture
//!
//! The `--nocapture` is useful: this is the canonical place to inspect
//! PyMapConv's own stdout/stderr until we build a UI to surface it.

use std::path::PathBuf;

use barme_core::{Heightmap, MapSize, Project};
use barme_pipeline::{CompileInputs, PyMapConvDriver};
use image::{ImageBuffer, Rgb};
use tracing_subscriber::EnvFilter;

/// Repo root, derived from CARGO_MANIFEST_DIR (which points at this crate).
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

#[test]
#[ignore = "needs vendored pymapconv (run scripts/fetch-pymapconv.sh)"]
fn pymapconv_compiles_minimum_legal_map() {
    init_tracing();

    let root = repo_root();
    let driver = PyMapConvDriver::vendored(&root)
        .expect("vendored pymapconv must be present; run scripts/fetch-pymapconv.sh");

    let workdir = tempfile::tempdir().expect("create tempdir");
    let work = workdir.path();

    // Smallest legal compile: 1024×1024 BMP + 129×129 16-bit heightmap.
    // PyMapConv infers SMF mapx = 1024/8 = 128 ⇒ heightmap dim 129 = BAR 2 SMU.
    let project = Project::new("smoke", 2);

    let hm_png = work.join("heightmap.png");
    Heightmap::synth_ramp(MapSize::square(2))
        .save_png(&hm_png)
        .expect("write 16-bit PNG");

    let tex_bmp = work.join("texture.bmp");
    write_grey_bmp(&tex_bmp, 1024, 1024, [128, 128, 128]);

    let out_dir = work.join("out");
    std::fs::create_dir_all(&out_dir).expect("create out_dir");

    let result = driver
        .compile(CompileInputs {
            project: &project,
            heightmap_png: &hm_png,
            texture_bmp: &tex_bmp,
            // C4 (Sprint 11): existing tests don't author metal spots,
            // so the metalmap PNG is not staged and PyMapConv's
            // default 1×1 black applies.
            metalmap_png: None,
            out_dir: &out_dir,
        })
        .unwrap_or_else(|e| panic!("compile failed:\n{e}"));

    // --nocapture surfaces these so the canonical pymapconv output is
    // visible in the test log.
    eprintln!("--- pymapconv stdout ---\n{}", result.stdout);
    eprintln!("--- pymapconv stderr ---\n{}", result.stderr);

    assert!(
        result.smf.is_file(),
        "smf missing at {}",
        result.smf.display()
    );
    assert!(
        result.smt.is_file(),
        "smt missing at {}",
        result.smt.display()
    );

    let smf_bytes = std::fs::metadata(&result.smf).unwrap().len();
    let smt_bytes = std::fs::metadata(&result.smt).unwrap().len();
    assert!(smf_bytes > 0, "smf is empty");
    assert!(smt_bytes > 0, "smt is empty");

    eprintln!("smf: {} ({} bytes)", result.smf.display(), smf_bytes);
    eprintln!("smt: {} ({} bytes)", result.smt.display(), smt_bytes);
}

fn write_grey_bmp(path: &std::path::Path, w: u32, h: u32, [r, g, b]: [u8; 3]) {
    let buf: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::from_pixel(w, h, Rgb([r, g, b]));
    buf.save(path).expect("write BMP");
}
