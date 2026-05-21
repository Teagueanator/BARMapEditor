# PITFALL §6 — mapinfo.lua silent dependencies

`mapinfo.lua` has a handful of inter-field rules whose violation
either silently degrades rendering or hard-crashes BAR's mod
gadgets. The lint pass catches each.

## Rules

- **`splatDetailNormalTex` without `specularTex`** — visibly flat
  output (FINDINGS §7.2). Engine no longer gates DNTS on
  `specularTex` at the C++ level (`SMFRenderState.cpp:114`), but
  the visual difference vs published BAR maps is still
  noticeable. The lint stays as a yellow warning; the build
  pipeline auto-bakes a grey 1024² specular fallback when the
  project doesn't supply one.
- **`voidWater = true` with `water.planeColor`** — mutually
  exclusive. The editor auto-clears `plane_color` when
  `void_water = true` and `warn!`s.
- **Missing or renamed `smtFileName0`** → the pink map. The
  emitter rewrites this on every build so the SMT and the
  mapinfo agree.
- **`fogStart == fogEnd`** — breaks the ground-grid renderer.
  Lint warns; the F9 form rejects the edit.
- **`extractor_radius = 500`** — engine default, BAR overrides
  to 80. 500 silently breaks mex-snap. The chip flags any value
  other than 80.

## Why three independent gates

mapinfo has three consumers — the engine scanner (ArchiveScanner,
extremely lax), the Chobby map browser (`modtype == 3` + a
certified-maps allowlist), and BAR's mod gadgets (which read
subtables directly without nil-checking). The emitter must
satisfy all three; the lint pass enforces the union.

See **Mapinfo three gates** in the user memory for the
breakdown — the conventional subtables grow as new gadget
nil-derefs surface.
