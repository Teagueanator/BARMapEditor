# Multithreading opportunities — BAR Map Editor

**Authored:** 2026-05-19 (post-Sprint-13).
**Status:** Research. No implementation work scheduled.
**Trigger to promote:** any of the per-bullet "When this becomes a
priority" gates fires (frame-rate complaint, build-time complaint,
profile evidence).

## TL;DR

Yes — there are real-world wins available from `rayon` (data-parallel
loops on multi-core CPUs) and from running build sub-processes in
parallel. None are blocking today. Order of expected impact:

1. **Procgen apply** — currently single-threaded per-pixel expression
   eval; a 4096² apply on a 4-core box would drop from ~600 ms to
   ~170 ms.
2. **`.sd7` build (SMF + SMT compile + 7z pack)** — PyMapConv runs
   serially; spawning SMF + SMT compiles in parallel + overlapping the
   metalmap / startboxes / mapinfo emission gets us ~30-40 % off
   wall-time.
3. **DNTS bake** — Compressonator subprocess per slot; up to 4 slots
   today (16 in Stage 2). Spawning them in parallel via a worker pool
   matches core count.
4. **Heightmap brush kernels** — CPU smooth at radius 1024 is 0.79 ms
   today (ADR-021 had 10× budget headroom); a 32×32 SMU map or a
   future erosion brush could push it past budget — `par_iter_mut` on
   rows is the fix.
5. **Layered painter composite** (Sprint 16+) — the CPU-side bake +
   tile-prep before GPU upload; speculative.

What's **not** worth it:
- Marker batch sort (sub-millisecond at <10k markers).
- GPU upload (`queue.write_*` is serialised by wgpu anyway).
- WGSL pipeline execution (already maximally parallel on the GPU).

## 1. Procgen apply (highest-impact opportunity)

### Current state

`barme-core::procgen::generate_heightmap` evaluates a user expression
(via `evalexpr`) once per pixel of a HEIGHTMAP × HEIGHTMAP grid. At
16 SMU (4097² = 16.78 M pixels) the cost is dominated by the
interpreter call. The Sprint 9 ADR-018 perf measurements suggest
single-thread procgen on a 4097² runs ~600-700 ms on a Ryzen 5800X3D.
The user sees an apply-button stall.

### Proposed change

```rust
use rayon::prelude::*;

pub fn generate_heightmap(expr: &str, dims: (u32, u32), domain: Domain)
    -> Result<Heightmap, ProcgenError>
{
    let context = parse_and_validate(expr)?;  // one-shot up front
    let (w, h) = dims;
    let mut data = vec![0u16; (w as usize) * (h as usize)];

    data.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(z, row)| {
            let mut local_context = context.clone();  // evalexpr Context is Send
            for (x, slot) in row.iter_mut().enumerate() {
                let value = eval_at(&mut local_context, x as f32, z as f32, domain);
                *slot = value;
            }
        });

    Ok(Heightmap { data, dims })
}
```

Each thread gets a fresh evalexpr `Context` (cheap clone — Variable +
function bindings only). The pixel writes are non-overlapping by row.

### Risk / pitfalls

- `evalexpr::Context` must be `Send + Sync`. Verify before committing.
  If not, wrap in `Arc<Mutex<>>` (defeats the purpose) — fall back to
  per-thread parsing of a pre-cleaned expression.
- Determinism: per-pixel eval has no inter-pixel dependency, so
  parallel evaluation yields byte-identical output.

### When this becomes a priority

A user reports the procgen apply stalling for >100 ms on a 16-SMU
map. Or 32-SMU support lands (32777² = 67 M pixels — single-thread
would cross the 2.5-second mark, parallel keeps it under 1 s).

## 2. `.sd7` build pipeline parallelism

### Current state

`barme-pipeline::sd7::build_sd7` runs serially:

1. Emit `mapinfo.lua` + `map_metal_layout.lua` + `map_startboxes.lua` +
   `featureplacer/*.lua` (CPU, fast).
2. Run PyMapConv to compile `.smf` + `.smt` (slow — minutes on big
   maps).
3. Optionally bake DNTS slot DDS files (slow — Compressonator per
   slot, ~5 s each, currently serialised inside PyMapConv anyway).
4. 7z non-solid pack of the staging dir (fast).

Steps 1, 4 are tens of ms each. Step 2 dominates. Step 3 is currently
sequenced inside Step 2.

### Proposed change

Replace the single PyMapConv invocation with two parallel
sub-process spawns:

```rust
use std::thread;

let smf_handle = thread::spawn(|| compile_smf_only(...));
let smt_handle = thread::spawn(|| compile_smt_only(...));
let dnts_handles: Vec<_> = active_slots.into_iter()
    .map(|slot| thread::spawn(move || bake_slot_dds(slot)))
    .collect();

let smf = smf_handle.join().unwrap()?;
let smt = smt_handle.join().unwrap()?;
let dnts_results = dnts_handles.into_iter()
    .map(|h| h.join().unwrap())
    .collect::<Result<Vec<_>, _>>()?;

stage_files(&smf, &smt, &dnts_results, ...);
seven_z_pack(...)?;
```

### Risk / pitfalls

- PyMapConv's CLI doesn't currently expose SMF-only / SMT-only flags.
  Would need an upstream patch OR vendoring a custom CLI shim — both
  add maintenance overhead. Likely not worth pursuing until the
  build-time complaint actually arrives.
- Disk I/O contention if multiple Compressonator processes thrash the
  same `tools/textures-cache/` directory; the SHA hashing in the
  cache key prevents collisions but the parallel disk writes might
  hot-spot the SSD. Mitigation: thread-pool sized at `num_cpus() / 2`
  rather than `num_cpus()`.

### When this becomes a priority

A user reports a `.sd7` build taking >30 s on a 16-SMU map (today
typically ~10-15 s). Or the layered painter (Sprint 17) ships with
16-slot DNTS support and serial bake takes 80 s — at that point the
parallelism is mandatory.

## 3. DNTS bake parallelism (subset of (2))

If we don't tackle the full (2), we can still parallelize JUST the
DNTS slot bakes today. `barme-pipeline::dnts::bake_dnts` is a single
slot's Compressonator subprocess + cache check. The `LayerStack` in
Sprint 17 will iterate over up to 16 active layers:

```rust
fn bake_all(active_layers: &[Layer]) -> Result<Vec<BakedDds>, BakeError> {
    active_layers.par_iter()
        .map(|layer| bake_dnts(&layer.spec, &cache_dir))
        .collect()
}
```

`rayon::par_iter` is fire-and-forget; results land in input order. The
cache hash prevents two threads racing on the same output file (it's
keyed by `sha256(input_bytes + bake_options)`; identical inputs
return the cached path immediately).

### When this becomes a priority

Sprint 17 (D10) ships. Multi-slot bakes become the norm.

## 4. Heightmap brush kernels (low priority today)

### Current state

`barme-core::brush::*` runs a single-threaded loop over the brush
footprint pixels:

```rust
for z in min_z..max_z {
    for x in min_x..max_x {
        let delta = kernel(distance(x, z, centre));
        heightmap[(z, x)] += delta;
    }
}
```

ADR-021 measured smooth-radius-1024 at 0.79 ms — well under the 8 ms
NFR. Headroom is ~10×.

### Proposed change (when needed)

```rust
use rayon::prelude::*;

heightmap.rows_mut(min_z..max_z)
    .par_bridge()  // or par_iter_mut over row slices
    .enumerate()
    .for_each(|(row_idx, row)| {
        let z = min_z + row_idx;
        for (col_idx, pixel) in row.iter_mut().enumerate() {
            let x = min_x + col_idx;
            let delta = kernel(distance(x, z, centre));
            *pixel = pixel.saturating_add(delta);
        }
    });
```

The row-major split keeps cache lines hot per thread. The DirtyRect
that the App tracks for GPU re-upload still works (union the per-
thread bboxes, or have each thread report its own and merge).

### Risk / pitfalls

- Symmetric brush replication writes N stamps per stroke (ADR-019).
  Each stamp has its own footprint that may overlap with the others;
  parallelizing within a stamp is fine, parallelizing across stamps
  is NOT (write races). Sequence the stamps, parallelize within each.
- Undo-snapshot bitset (ADR-033) is shared mutable state. Parallel
  per-row writes to it need either a per-thread bitset that merges at
  end-of-stamp, or atomic word-level CAS. Test carefully.

### When this becomes a priority

A user reports stroke lag at 32×32 SMU. Or a new erosion / slope
brush lands with a >1 ms per-stamp cost.

## 5. Layered painter CPU prep (speculative)

Sprint 16 (D9) is the GPU layered composite pipeline. Sprint 17 (D10)
adds DNTS hybrid emission. CPU work in those sprints includes:

- Box-downsampling 4096² masks to 1024² for the splat distribution
  PNG.
- Building the per-tile dirty-rect upload lists.
- Pre-baking the diffuse BMP at full texture resolution for the
  `.sd7` output (Sprint 17's bake step).

Each is row-decomposable and worth a `par_iter_mut` once the sprints
ship. Don't over-engineer before the perf data exists.

## What multithreading does NOT buy us

- **Marker batch sort.** Sub-millisecond at <10k markers (Sprint 13
  measurement). Rayon's overhead would dominate the work. Sprint 24
  (S3O features) might push counts toward 50k, at which point a
  `par_sort_by_cached_key` is a 1-line swap (escape hatch
  documented in `ui/markers.rs`).
- **GPU pipeline execution.** wgsl shaders are already maximally
  parallel — CPU threads don't accelerate GPU work.
- **GPU buffer uploads.** `wgpu::Queue::write_buffer` /
  `write_texture` are serialised through wgpu's internal command
  queue. Calling them from multiple threads serialises behind a
  mutex; no parallelism win.
- **egui repaint.** Single-threaded by design.

## Library choice

[`rayon`](https://docs.rs/rayon) is the standard answer for
data-parallel loops in Rust. It's:
- Permissively licensed (Apache-2.0 / MIT).
- Used by many established Rust projects (rustfmt, ripgrep,
  rust-analyzer).
- A small footprint (~20k LoC, no heavy deps).
- Composable with `Iterator` via `par_iter` / `par_iter_mut`.

For sub-process orchestration (`.sd7` build, DNTS bake), plain
`std::thread::spawn` + `JoinHandle::join` is sufficient — no extra
dep needed. If we ever want a worker pool that survives across many
short tasks, `rayon::ThreadPoolBuilder::new()` is the simplest path.

## Order of attack (if/when scheduled)

If the team wants to schedule a multithreading sprint, here's the
recommended order:

1. **Procgen** (smallest, highest immediate user-visible win).
2. **DNTS bake parallelism** (drops naturally into Sprint 17's bake
   refactor; one `par_iter` line).
3. **Heightmap brushes** (only when 32-SMU support arrives or a
   user complains).
4. **`.sd7` SMF + SMT split** (only after the PyMapConv upstream
   feature lands or we vendor a CLI shim).
5. **Layered painter prep** (defer until Sprint 16/17 ship and we
   have profile data).

Each item is small enough (~150-300 LoC + tests) to fit a half-sprint
slot without disrupting the Sprint 14+ feature pipeline.
