# Fixtures

Heightmap/texture fixtures for tests and manual smoke runs.

**These files are gitignored.** See `docs/DECISIONS.md` ADR-007 for the
rationale (repo bloat, diff noise, fixture spec belongs in code).

## Regenerate

```bash
cargo run -p barme-core --example gen-fixture
```

This writes deterministic 16-bit grayscale ramps at three SMU sizes:

| File                            | SMU  | Heightmap dims |
|---------------------------------|------|----------------|
| `r16_ramp_2x2smu_129px.png`     | 2×2  | 129 × 129      |
| `r16_ramp_4x4smu_257px.png`     | 4×4  | 257 × 257      |
| `r16_ramp_16x16smu_1025px.png`  | 16×16| 1025 × 1025    |

Heightmap edge length is always `64·N + 1` — the off-by-one is the #1
silent corruption in this file format (see `docs/PITFALLS.md`).
