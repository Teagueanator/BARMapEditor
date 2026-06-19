//! Procgen apply latency bench (Sprint 33 / T6 — promotes the Sprint-24
//! `examples/bench_procgen.rs` into a CI-gated criterion bench).
//!
//! Guards the Sprint-24 multithreading target: a full 16-SMU parabolic
//! `procgen::generate` must stay ≤ 250 ms on a 4-core box. Criterion
//! reports the median; CI fails the PR on a >1.5× regression. The hard
//! ceiling is asserted CI-safely in `procgen::tests`
//! (`generate_16_smu_parabolic_parallel_under_400ms`); this bench tracks
//! the trend.
//!
//!     cargo bench -p barme-core --bench procgen_apply

use std::hint::black_box;

use barme_core::{MapSize, procgen};
use criterion::{Criterion, criterion_group, criterion_main};

fn bench_procgen(c: &mut Criterion) {
    let mut group = c.benchmark_group("procgen_generate");
    // Long apply on a 1025² grid → keep the sample count modest so CI
    // wall-time stays bounded.
    group.sample_size(20);

    let size = MapSize::square(16);

    // Two cost shapes: pure-arith parabolic vs sqrt-bearing cone.
    let cases: &[(&str, procgen::Domain, &str)] = &[
        ("parabolic", procgen::Domain::Centered, "1 - (x*x + z*z)"),
        (
            "cone_peak",
            procgen::Domain::Centered,
            "max(0, 1 - math::sqrt(x*x + z*z))",
        ),
    ];

    for (label, domain, expr) in cases {
        group.bench_function(*label, |b| {
            b.iter(|| {
                let _ =
                    procgen::generate(black_box(expr), *domain, black_box(size), 0.0, 1.0).unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_procgen);
criterion_main!(benches);
