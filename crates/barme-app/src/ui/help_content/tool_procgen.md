# Procgen

Math-function heightmap generator. Type a formula `f(x, z) →
[0, 1]` and commit it to the heightmap. Faster than sculpting
for base elevation; finish in Sculpt for the fine details.

## Domain

Two choices:

- **Unit** — `x` and `z` run `0..1` across the map. Good for
  one-sided ramps and corner-anchored shapes.
- **Centered** — `x` and `z` run `-1..1` from the map centre.
  Good for radial / dish-shaped expressions like
  `1 - x*x - z*z`.

## Presets

Stock expressions seeded in the Inspector:

- **Parabolic bowl** — central depression. Pair with sculpt
  smoothing for natural lakebeds.
- **Saddle** — pass. Useful for mountain-defile maps.
- **Diagonal ramp** — monotonic slope.
- **Plateau** — elevated mesa.
- **Custom** — your own expression.

## Live preview

A 256² greyscale thumbnail in the Inspector refreshes ~50 ms
after the last keystroke. The green chip means the expression
parses; the red chip means a parse error (hover for details).
Commit is disabled while red.

## Apply is undoable

Clicking Apply replaces the current heightmap. `Ctrl+Z` reverts.
The undo entry is a copy-on-first-write snapshot of the full
heightmap, bounded by the project's pixel count (≤ ~2 MB at 16
SMU).
