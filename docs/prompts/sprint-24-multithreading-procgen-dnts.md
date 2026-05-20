# Sprint 24 — Multithreading: rayon procgen + parallel DNTS bake (T2)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 24** — the first multithreading sprint. Two
opportunities have crossed their promotion gates per
`docs/research/multithreading/PROPOSAL.md`:

1. **Procgen apply** — single-threaded per-pixel expression eval
   takes ~600-700 ms on a 16-SMU map. Sprint 17 lands the layered
   painter; the procgen workflow ("press G, pick a preset, tweak,
   apply") is now the heaviest UI stall in the editor. **Target:
   ~170 ms on a 4-core box for the same 16-SMU map** via
   `rayon::par_chunks_mut`.

2. **DNTS bake parallelism** — Sprint 17 (D10) added 16-layer
   support; bake-time scales linearly today (4 layers × ~5 s =
   20 s in the build path). **Target: bake time = max(slot_time)
   instead of sum**, via `rayon::par_iter` over active DNTS-bound
   layers. With a 4-core box and 16 slots, the 80 s serial bake
   drops to ~25 s.

This sprint adds `rayon` as a workspace dep, applies it
**targeted-not-global**, and writes regression benchmarks that
catch perf drift.

**Prerequisites:**
- Sprint 23 (painter cleanup) MUST be ticked. 16-SMU memory
  budget is stable before we add concurrent allocations.
- Sprint 20 (async build pipeline) MUST be ticked. The DNTS bake
  parallelism lives inside the worker thread that Sprint 20
  established.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.3 NFR-Performance
   (≤8 ms brush latency, ≤1 s procgen apply, ≤30 s `.sd7` build
   on 16-SMU).
3. **`/home/teague/code/BARMapEditor/docs/research/multithreading/PROPOSAL.md`**
   — full proposal. Section 1 (procgen) and Section 3 (DNTS bake)
   are this sprint's scope. Sections 2 (`.sd7` SMF/SMT split) and
   4 (brush kernels) stay deferred. Section 5 (layered painter
   prep) is partially mooted by Sprint 23's cleanup.
4. `/home/teague/code/BARMapEditor/crates/barme-core/src/procgen.rs`
   — single-threaded `generate_heightmap` + `generate_thumbnail`.
5. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/dnts.rs`
   — `bake_dnts` per slot.
6. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/splat_pipeline.rs`
   — Sprint 17's `stage_splat_assets_from_layers` calls `bake_dnts`
   per active DNTS-bound layer.
7. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/build.rs`
   (Sprint 20) — the build orchestrator that emits per-stage
   events. The new parallel bake emits sub-progress
   (`BuildEvent::Progress(f32)` per completed slot).

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-24-procgen-rayon
./devlog/log.sh new sprint-24-dnts-bake-parallel
```

## Step 3 — Scope

In order, one commit per item:

### 1. Add `rayon` to workspace deps + procgen apply parallelism

**Workspace `Cargo.toml`**:
```toml
[workspace.dependencies]
rayon = "1.10"
```

**`crates/barme-core/Cargo.toml`** + **`crates/barme-pipeline/Cargo.toml`**:
add `rayon = { workspace = true }`.

**`crates/barme-core/src/procgen.rs`** — `generate_heightmap`:

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
        .try_for_each(|(z, row)| -> Result<(), ProcgenError> {
            let mut local_context = context.clone();  // evalexpr Context is Send
            for (x, slot) in row.iter_mut().enumerate() {
                let value = eval_at(&mut local_context, x as f32, z as f32, domain)?;
                *slot = value;
            }
            Ok(())
        })?;

    Ok(Heightmap { data, dims })
}
```

**Pre-flight check** (do this before writing the code):
1. Verify `evalexpr::Context` is `Send + Clone`. If not, plan B is
   per-thread parsing of a pre-cleaned expression. Document in the
   devlog log either way.
2. Determinism: per-pixel eval has no inter-pixel dependency, so
   parallel evaluation yields byte-identical output. Add a
   regression test that compares serial vs parallel output on the
   default presets.

**`generate_thumbnail`** stays single-threaded (256² is small;
parallel overhead dominates).

**Benchmark** (`crates/barme-core/examples/bench_procgen.rs`,
new or extended):

```rust
fn main() {
    for smu in [4u32, 8, 12, 16] {
        let dim = smu * 64 + 1;
        let start = Instant::now();
        let _ = generate_heightmap("parabolic", (dim, dim), Domain::Unit).unwrap();
        println!("{}-SMU: {:.0}ms", smu, start.elapsed().as_secs_f64() * 1000.0);
    }
}
```

Record the bench results in the devlog. Target: 16-SMU apply
< 250 ms on a 4-core dev box. Add a CI bench (slow lane) that
fails if the regression budget creeps past 1.5× of the baseline.

### 2. Parallel DNTS bake

**`crates/barme-pipeline/src/splat_pipeline.rs`** —
`stage_splat_assets_from_layers`:

```rust
use rayon::prelude::*;

fn stage_splat_assets_from_layers(
    layers: &LayerStack,
    project_root: &Path,
    bake_cache: &Path,
    on_progress: &dyn Fn(usize, usize),  // (completed, total)
) -> Result<Vec<BakedDds>, BakeError> {
    let dnts_layers: Vec<_> = layers.dnts_layers().collect();
    let total = dnts_layers.len();
    let completed = AtomicUsize::new(0);

    let results: Vec<Result<BakedDds, BakeError>> = dnts_layers
        .par_iter()
        .map(|layer| {
            let result = bake_dnts(&layer.spec, project_root, bake_cache);
            let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
            on_progress(done, total);
            result
        })
        .collect();

    results.into_iter().collect()
}
```

The `bake_dnts` cache is content-addressed (sha256(input_bytes +
options)); identical inputs return the cached path immediately,
so parallel threads racing on the same output are safe.

**Sub-progress events** to the Sprint 20 build pipeline:

```rust
BuildEvent::Stage(BuildStage::BakeDnts {
    slot_index: done,
    total_slots: total,
});
```

The build overlay's progress bar (Sprint 20) reflects completed
slots. The progress callback fires per-slot completion, not per-
slot start (start-time ordering is non-deterministic with rayon).

**Thread-pool sizing**: rayon's default = num_cpus(). For DNTS bake
this hits Compressonator subprocess concurrency limits on slower
disks. Cap at `min(num_cpus, 4)` for the bake — set via
`rayon::ThreadPoolBuilder::new().num_threads(N).build_scoped(...)`.

### 3. Brush-kernel readiness (deferred but scoped)

Per PROPOSAL §4, brush kernels stay single-threaded for Sprint 24.
However, the **promotion gate** ("32-SMU support arrives or user
reports stroke lag") is approaching — add a `// TODO(sprint-MT-brushes)`
marker at `crates/barme-core/src/brushes/{raise,lower,smooth}.rs`'s
inner loop. Document the rayon pattern in the comment so a future
sprint is a one-line lift.

Do NOT parallelise here. The 0.79 ms baseline (ADR-021) is well
within budget.

### 4. Tests + bench + rollup

- **Determinism test** (`procgen::tests::par_serial_byte_identical`):
  serial vs parallel output is byte-identical across all 5
  default presets at 4-SMU and 16-SMU.
- **DNTS bake parallelism test**: stage 4 distinct layers,
  measure wall-time, assert wall-time < 1.5× max(per-layer
  bake time). Hard to make this deterministic (subprocess
  timing varies); a soft assertion + log is fine.
- **Bench** (`crates/barme-core/examples/bench_procgen.rs`):
  prints timings, used manually.
- **Regression**: `cargo test --workspace` green; clippy clean.
- **Rollup commit**: STATUS UPDATEs in SRS / ROADMAP (T2 done +
  NFR-Performance for procgen tightened); closing devlog logs;
  "Sprint 25 = terrain shader parity (port SMFFragProg)"
  handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on procgen apply with
`elapsed_ms` and `cores_used`; `info!` on DNTS bake with
`slots_baked, elapsed_ms`. `warn!` if `evalexpr::Context::clone`
fails its `Send` bound (fallback to per-thread parse).

## Step 5 — Out of scope

- **`.sd7` SMF/SMT split parallelism** — PROPOSAL §2. Waits on
  PyMapConv upstream patch or a vendored CLI shim. Not this sprint.
- **Brush kernel parallelism** — PROPOSAL §4. Not gated yet.
- **Layered painter CPU prep parallelism** — PROPOSAL §5. Sprint
  17 already used `par_iter_mut` for bake_diffuse; nothing new
  here.
- **WGPU multithreaded encoding** — PROPOSAL TL;DR notes this
  is not worth it (write_* serialises through internal mutex).
- **Lint pass parallelism** (Sprint 21 mentions this) — defer
  until lint actually exceeds 16 ms.

## Step 6 — Critical pitfalls (read twice)

1. **`evalexpr::Context` `Send + Clone` check**: pin this in a
   compile-time `fn assert_send<T: Send>() {}` call at the top of
   `procgen.rs`. If it ever loses `Send`, the build fails fast
   instead of silently regressing.

2. **Determinism**: per-pixel eval is order-independent IF the
   expression has no side effects. `evalexpr` is pure by default;
   but a future custom function (`noise()`) might introduce state.
   The determinism test catches drift.

3. **Rayon overhead at small sizes**: for the 256² thumbnail
   path, parallel is **slower** than serial. Keep
   `generate_thumbnail` single-threaded and document why.

4. **Compressonator concurrency**: spawning 16 subprocesses
   simultaneously on a 4-core box thrashes the CPU. The cap at
   `min(num_cpus, 4)` is the safe default. Test on a 16-SMU
   project with 16 DNTS-bound layers; bake time should drop
   ~4× not 16×.

5. **Cache key races**: `bake_dnts`'s content-addressed cache
   uses atomic file writes (`tempfile + rename`). Two threads
   racing on the same input write to two distinct temp files,
   then both rename to the same target — atomic rename means
   the last writer wins, but the file is always valid. Verify
   by reading `dnts.rs`'s cache code; if it's not atomic, fix
   it here.

6. **Progress event ordering**: rayon's `par_iter` doesn't
   guarantee completion order. The progress callback fires per-
   slot completion; the build overlay receives unordered (3/8,
   1/8, 4/8, 2/8, ...). The overlay's progress bar uses
   `completed_count / total`, not per-slot ordering, so this is
   fine. Don't sort.

7. **`AtomicUsize` for completion count**: use `Ordering::SeqCst`
   for the counter; it's a coarse counter, not a hot path.

8. **Rayon thread-pool global vs scoped**: use a SCOPED thread
   pool for DNTS bake (`build_scoped`) — don't pollute the
   global pool with the 4-cap. Procgen apply uses the default
   global pool (which the rest of the workspace can also use).

9. **CPU-affinity concerns on Windows**: rayon respects logical
   cores by default. On systems with E-cores (Alder Lake), rayon
   doesn't pin to P-cores. Acceptable for v1; Stage 2 polish
   may revisit.

10. **`procgen_last_error` UI**: the existing error path in
    `main.rs` shows the eval error in the procgen inspector.
    If `try_for_each` aborts on the first error, the error
    surface remains identical — verify with a fixture that
    has a divide-by-zero at pixel (500, 500).

11. **Bench reproducibility**: pinning a benchmark to a CI box
    is fragile. The CI bench (slow lane) uses GitHub Actions
    runners which have variable load. Report a 5-run median;
    fail only if median > 1.5× baseline.

## Step 7 — Exit criteria

- 4+ commits on `main`: deps + procgen, DNTS bake parallel,
  bench harness, brush TODO marker (small commit), rollup.
- 2 devlog folders filled.
- SRS / ROADMAP STATUS UPDATEs (T2 + NFR-Performance procgen
  budget tightened).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test:
  - 16-SMU procgen apply (parabolic) wall-time on dev box:
    ≤ 250 ms (was ~600 ms).
  - 16-DNTS-layer build: bake wall-time ≤ 1.5× max(per-slot
    bake), down from N × max(per-slot bake).
  - Determinism test passes.
- Final devlog: summary + "Sprint 25 = terrain shader parity"
  handoff.

Start by verifying `evalexpr::Context: Send + Clone`. If yes,
the procgen lift is mechanical. If no, document the workaround
in the devlog and ship the per-thread parse fallback.
