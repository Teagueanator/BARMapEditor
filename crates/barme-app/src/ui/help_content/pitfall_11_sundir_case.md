# PITFALL §11 — `sundir` vs `sunDir` case mismatch

Lua tables are case-sensitive. Two consumers read the same
field with different capitalisation:

- **Engine** (`rts/Map/MapInfo.cpp:207`) reads ONLY camelCase
  `lighting.sunDir`.
- **BAR's active `luarules/gadgets/unit_sunfacing.lua`** (March
  2024, line 43) reads ONLY lowercase `lighting.sundir`.

If you emit only one, the other consumer fails silently —
either lighting renders flat (engine path missing) or units
ignore sun-facing in the gameplay loop (gadget path missing).

## Rule

The emitter writes BOTH keys with the same value into the
`lighting` subtable. A unit test in
`barme-pipeline::mapinfo::tests` asserts both keys appear in
the rendered output.

## Why this isn't a single key

Spring's engine code historically used `sunDir` (camelCase per
the C++ convention); BAR's Lua gameplay code adopted `sundir`
(lower-case per a different convention). Neither side will
change first. The pragmatic fix is to ship both.

## Other case-mismatches

`atmosphere.skyDir` is **deprecated** in favour of
`atmosphere.skyAxisAngle` — see PITFALL §12. Don't emit
`skyDir`; lint warns if user-edited mapinfo overrides set it.
