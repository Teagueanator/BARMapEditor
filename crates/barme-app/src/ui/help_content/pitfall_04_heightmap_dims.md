# PITFALL §4 — Heightmap dims must be `64·N + 1`

The #1 silent corruption mode. Spring's heightmap is a `short[]`
array of size `(mapy+1) * (mapx+1)` where `mapx = 64 · SMU_x` and
`mapy = 64 · SMU_z`. So a 16×16 SMU map needs **`1025 × 1025`**
heightmap pixels — NOT 1024×1024.

If you import a power-of-two heightmap, PyMapConv silently warns
and resizes. The terrain in BAR ends up offset and the corners
clip wrong. The editor's import path rejects with an explicit
error rather than silently cropping or padding.

## Rule

`MapSize::heightmap_dims()` is the only place dims are computed.
Imports validate to `64·N + 1`; rejection is fatal.

## Resizing your input

If your World Machine / Substance output is 1024², open it in
any image editor and pad to 1025² with the bottom row +
right-most column duplicated. Or use the wizard's biome preset +
sculpt the difference.

For procgen projects this is hidden by `MapSize` — the math
inside `procgen::generate` honours `64·N + 1` automatically.
