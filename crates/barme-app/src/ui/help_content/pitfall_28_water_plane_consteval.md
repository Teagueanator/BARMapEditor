# PITFALL §28 — BAR's water plane is `consteval`; users CANNOT move it

`RecoilEngine/rts/Map/Ground.h:23-38` makes
`GetWaterPlaneLevel()` a `consteval` function returning `0.0`.
**Water is always at world Y = 0. You can't raise or lower it.**

A "water depth" or "water level" slider in the editor would be
a lie. The right affordance is a `Project.min_height` control
(negative = the lowest heightmap sample sits below sea level,
flooding the basin).

## Rule

The Water tool's Inspector exposes `min_height` directly. There
is no "water level" slider — that field doesn't exist in BAR.

The auto-min-height shortcut sets
`min_height = min(0, carve_depth)` so a Water-tool LMB-drag
immediately produces visible water in the carved basin.

## Renderer regression watch

The terrain shader's `sample_y` MUST consult `Project.min_height`
when projecting raw heightmap values to world Y. A Sprint 14
regression mapped raw `u16` linearly into `[0, max_height]`,
ignoring `min_height` entirely — even with `min_height = -100`
the heightmap rendered as if it started at `Y = 0`, so the
water plane sat flush with the floor and was invisible. Fixed
by extending the terrain Uniforms with `params2.x = min_height`
and updating `sample_y` to compute
`y = min_h + t * (max_h - min_h)`.

Pinned by the C9 inspector emission tests + the shader uniform
pin in `render.rs::tests`.

## Workaround for users

If you want a "high water" effect (water reaching a mesa top):

1. Lower the entire heightmap so the mesa is lower than the
   "high water" line you imagined.
2. Or, paint a high mesa with the basin around it carved
   deep — visually equivalent, simpler to author.

The water plane is decoupled from terrain elevation; designing
around `min_height < 0` is the only correct path.
