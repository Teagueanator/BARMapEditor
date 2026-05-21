# PITFALL §13 — SMF metalmap must be all-zero when emitting Lua metal spots

BAR's `map_metal_spot_placer.lua` gadget reads spots from the
engine metalmap at startup; **if any pixel is non-zero, the
gadget bails and the Lua-defined spots in
`mapconfig/map_metal_layout.lua` are ignored**.

So you can't combine the two systems. Either you place every
mex via the engine metalmap PNG (a `(32 · N)²` 8-bit greyscale
image — red channel = density), or you place them via the Lua
file. The editor's Sprint 11 work picked Lua: it gives you
named-spot semantics (yield per spot, no clustering radius
auto-derived) and matches what real BAR maps do.

## Rule

The build pipeline ships an all-zero `(32 · smu_x) × (32 ·
smu_z)` metalmap PNG to PyMapConv whenever `Project.metal_spots`
is non-empty. Integration test: after build, load the `.sd7`'s
SMF, read the metalmap region, assert all bytes are zero.

## What if I want PNG-defined spots

Don't, if your map ships in BAR. The PNG path is a Zero-K /
Spring legacy convention; BAR's `gui_metalspots` widget reads
the Lua file exclusively. If you really need the PNG path:

1. Set `Project.metal_spots = vec![]`.
2. Author the metalmap PNG by hand and import it via the F9
   form's resources tab.
3. Accept that F4 in-game won't show predicted yields.

The lint pass will warn if you ship both.
