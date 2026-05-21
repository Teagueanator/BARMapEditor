//! Sprint 24 (T2): wall-time bench for `procgen::generate` across
//! 4 / 8 / 12 / 16 SMU. Records the dev-box numbers for the devlog —
//! the in-tree perf assertion at
//! `procgen::tests::generate_16_smu_parabolic_parallel_under_400ms`
//! is a CI-safe ceiling; the hard PROPOSAL §1 target ("<250 ms on a
//! 4-core box, 16 SMU") gets checked here by hand.
//!
//! `cargo run --example bench-procgen --release -p barme-core`
//!
//! Reports 5-run median per (SMU, preset). Median over mean dampens
//! the one-shot variance from scheduler jitter; the bench is meant to
//! catch a 1.5× regression, not nanosecond-tight micro-bench drift.

use std::time::Instant;

use barme_core::{MapSize, procgen};

fn main() {
    let cores = rayon::current_num_threads();
    println!("procgen bench (rayon threads = {cores})");
    println!("{:->68}", "");
    println!(
        "{:>6}  {:>14}  {:>22}  {:>14}",
        "SMU", "dims", "preset", "median ms"
    );
    println!("{:->68}", "");

    // Two presets exercise the two cost shapes:
    //   parabolic-bowl  — pure arith (Mul/Add)
    //   cone-peak       — adds an sqrt (a `Function` dispatch + math::sqrt)
    let presets: &[(&str, procgen::Domain, &str)] = &[
        (
            "parabolic-bowl",
            procgen::Domain::Centered,
            "1 - (x*x + z*z)",
        ),
        (
            "cone-peak",
            procgen::Domain::Centered,
            "max(0, 1 - math::sqrt(x*x + z*z))",
        ),
    ];

    for smu in [4u32, 8, 12, 16] {
        let size = MapSize::square(smu);
        let (w, h) = size.heightmap_dims();
        for (label, domain, expr) in presets {
            // Warm-up: page caches + rayon thread-pool init. Discarded.
            let _ = procgen::generate(expr, *domain, size, 0.0, 1.0).unwrap();

            let mut samples_ms = Vec::with_capacity(5);
            for _ in 0..5 {
                let t0 = Instant::now();
                let _ = procgen::generate(expr, *domain, size, 0.0, 1.0).unwrap();
                samples_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            }
            samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = samples_ms[samples_ms.len() / 2];
            println!(
                "{:>5} {:>14}  {:>22}  {:>11.1} ms",
                smu,
                format!("{w}×{h}"),
                label,
                median
            );
        }
    }

    println!("{:->68}", "");
    println!(
        "Reference: PROPOSAL §1 target = <250 ms at 16 SMU on a 4-core box.\n\
         Sprint 23 baseline (serial path, pre-Sprint-24): ~440 ms parabolic."
    );
}
