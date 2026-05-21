# PITFALL §8 — DNTS + water + LOS animated-snow bug

A subtle interaction between three independently-correct systems
produces visible artifacts. Documented by Beherith in the
springrts forum thread t=35202.

## Reproduction

- `minHeight < 0` (so the map dips below sea level), AND
- DNTS layers are active, AND
- A Lua widget calls into the LOS API (`Spring.GetLineOfSight`).

The terrain renders correctly statically but turns into
"animated TV-snow" — flickering, high-frequency colour noise —
under camera motion.

## Rule

Warn (don't block) when DNTS is enabled on a map with
`minHeight < 0`. The lint pass surfaces this as a yellow
warning.

## Workaround

If the artifact appears in BAR:

1. Set `min_height = 0` (sea level == lowest sample). The flood
   workflow no longer works but DNTS stops misrendering.
2. Or disable DNTS — paint with diffuse only via the Paint layer
   stack and skip the per-channel normal maps.
3. Or contact the BAR rendering team — this is an upstream bug
   in the engine, not the editor.

## Renderer-parity arc note

The Sprint 23+ renderer-parity arc plans to reproduce BAR's
ground shader closely enough that we'd see this same artifact
in the editor preview. Until then, the editor's flat DNTS
preview doesn't trigger the bug.
