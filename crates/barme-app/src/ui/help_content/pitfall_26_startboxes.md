# PITFALL §26 — `map_startboxes.lua`: existence beats content, shape is unwrapped

Two related findings from the 2026-05-19 live-BAR smoke test.
Either trap silently breaks start boxes.

## Existence beats content

`luarules/gadgets/include/startbox_utilities.lua::ParseBoxes:43`
checks `VFS.FileExists("mapconfig/map_startboxes.lua")` and uses
the file's return value as-is. There is no "is this table
empty?" check.

So shipping an empty `map_startboxes.lua` **suppresses** BAR's
default-fallback codepath (lines 79–137 of the same file), which
would otherwise generate sensible N/S or E/W boxes from map
dimensions. **An absent file is strictly better than an empty
one.**

## Rule

`startboxes::should_emit(project)` is `true` only when the
project has ≥2 ally groups AND at least one has an authored
`box_polygon`. `build_sd7` skips staging the file otherwise; BAR's
fallback then applies.

## Shape is unwrapped, in elmos

The file returns the per-ally-team table **directly** — not
wrapped in `{ startboxes = {…} }`. Polygon vertices are in
**elmo coordinates** (not 0..1 fractions):

```lua
return {
  [0] = {
    nameLong = "North-West", nameShort = "NW",
    boxes       = { { {0,0}, {614,0}, {614,614}, {0,614} } },
    startpoints = { {307, 307} },
  },
  [1] = { … },
}
```

The modoptions-string codepath at lines 56-59 *does* multiply
fractions by `Game.mapSizeX/Z`, but the map-file codepath does
not. **Conflating those two formats yields silently-broken
boxes.**

## Diagnosis

If "Recommended spawn area" doesn't show in BAR's skirmish
lobby:

1. Open the `.sd7`. Is `mapconfig/map_startboxes.lua` present?
2. If yes and `boxes = { … }` is empty, delete the file —
   BAR's fallback will work.
3. If polygon coords are < 100, you probably used fractions.
   Re-author with elmo coordinates.
4. Re-build from the editor; the should_emit gate handles this
   automatically.
