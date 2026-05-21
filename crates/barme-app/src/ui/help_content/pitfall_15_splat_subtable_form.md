# PITFALL §15 — `splatDetailNormalTex` prefers the subtable form

The engine reader (`MapInfo.cpp:383-399`) checks for the subtable
form of `splatDetailNormalTex` first and only falls back to the
legacy numbered keys (`splatDetailNormalTex1`,
`splatDetailNormalTex2`, …) if absent. Mixing both will result
in the subtable form winning silently.

## Rule

Emit the subtable form only:

```lua
resources.splatDetailNormalTex = {
  "tex1.dds", "tex2.dds", "tex3.dds", "tex4.dds",
  alpha = (true|false),  -- == splatDetailNormalDiffuseAlpha
}
```

The `alpha` keyed field controls whether the bound DDSes' alpha
channels carry diffuse colour (the D8 "diffuse-in-alpha" bake
optimization).

## Why the editor goes one-way

Sprint 12 / D6 introduced a `LuaValue::Mixed { values, keyed }`
AST node specifically to render this bare-positional +
keyed-trailer shape. The schema still parses
`splatDetailNormalTex1..4` on import for hand-authored map
survival, but the D6 emitter never writes the legacy form.

Regression test
`barme_pipeline::mapinfo::resources_subtable_form_not_legacy`
pins both halves: subtable present, legacy numbered keys absent.

## What you'll see in the .sd7

Open the emitted mapinfo.lua. The `resources` block contains:

```lua
splatDetailNormalTex = {
  "maps/textures/grass_dnts.dds",
  "maps/textures/rock_dnts.dds",
  "maps/textures/sand_dnts.dds",
  "maps/textures/snow_dnts.dds",
  alpha = false,
}
```

Each path resolves to a per-active-channel DDS staged by the
splat pipeline (D6).
