//! Quick latency bench for CPU brush kernels at 16 SMU.
//! Sanity check against SRS NFR-Performance ≤ 8 ms per stamp.
//!
//! `cargo run --example bench-brushes --release -p barme-core`

use std::time::Instant;

use barme_core::{Brush, BrushStamp, Heightmap, MapSize, brushes};

fn main() {
    let size = MapSize::square(16);
    let mut hm = Heightmap::synth_ramp(size);
    let (w, h) = hm.dims();
    println!(
        "16 SMU heightmap: {}×{} = {} px ({:.1} MB u16)",
        w,
        h,
        (w as u64 * h as u64),
        (w as f64 * h as f64 * 2.0) / 1_048_576.0
    );

    let mid_x = ((w - 1) as f32 * 8.0) * 0.5;
    let mid_z = ((h - 1) as f32 * 8.0) * 0.5;

    for radius in [128.0f32, 256.0, 512.0, 1024.0] {
        let stamp = BrushStamp {
            world_x: mid_x,
            world_z: mid_z,
            radius,
            strength: 0.5,
        };
        // Warm up
        for _ in 0..3 {
            let _ = brushes::Raise.apply(&mut hm, stamp);
        }
        const N: u32 = 50;
        for (name, brush) in [
            ("raise", &brushes::Raise as &dyn Brush),
            ("lower", &brushes::Lower as &dyn Brush),
            ("smooth", &brushes::Smooth as &dyn Brush),
        ] {
            let t0 = Instant::now();
            for _ in 0..N {
                let _ = brush.apply(&mut hm, stamp);
            }
            let dt = t0.elapsed();
            let per = dt.as_secs_f64() * 1000.0 / N as f64;
            println!("  r={radius:6.0} elmos  {name:6}: {per:8.3} ms/stamp");
        }
    }
}
