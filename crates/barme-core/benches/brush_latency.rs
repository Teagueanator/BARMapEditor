//! Brush-stroke latency bench (Sprint 33 / T6 — promotes the Sprint-1
//! `examples/bench_brushes.rs` into a CI-gated criterion bench).
//!
//! Guards NFR-Performance: a single brush stamp on a 16-SMU heightmap
//! must stay ≤ 8 ms. Criterion reports the *median* and CI fails the PR
//! on a >1.5× regression vs the cached baseline (see ci.yml). The hard
//! 8 ms ceiling itself is asserted as a CI-safe unit test in
//! `brushes::tests` (`smooth_stamp_16_smu_under_budget`); this bench is
//! the trend/regression instrument.
//!
//!     cargo bench -p barme-core --bench brush_latency

use std::hint::black_box;

use barme_core::{Brush, BrushStamp, Heightmap, MapSize, brushes};
use criterion::{Criterion, criterion_group, criterion_main};

fn bench_brushes(c: &mut Criterion) {
    let size = MapSize::square(16);
    let mut hm = Heightmap::synth_ramp(size);
    let (w, h) = hm.dims();
    let mid_x = ((w - 1) as f32 * 8.0) * 0.5;
    let mid_z = ((h - 1) as f32 * 8.0) * 0.5;

    let mut group = c.benchmark_group("brush_stamp_16smu");

    // The worst case is the largest radius (most pixels touched) and the
    // smooth kernel (a 3×3 read per write). r=1024 elmos is the
    // NFR-Performance reference radius.
    for radius in [256.0f32, 512.0, 1024.0] {
        let stamp = BrushStamp {
            world_x: mid_x,
            world_z: mid_z,
            radius,
            strength: 0.5,
        };
        for (name, brush) in [
            ("raise", &brushes::Raise as &dyn Brush),
            ("smooth", &brushes::Smooth as &dyn Brush),
        ] {
            group.bench_function(format!("{name}_r{radius:.0}"), |b| {
                b.iter(|| {
                    let _ = brush.apply(black_box(&mut hm), black_box(stamp));
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, bench_brushes);
criterion_main!(benches);
