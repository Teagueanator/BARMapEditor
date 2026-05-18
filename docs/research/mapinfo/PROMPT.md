# Research prompt — BAR mapinfo.lua schema + gadget conventions

**Use:** Paste the section below verbatim into a fresh Claude deep-research
session.

**Expected output:** a reference document — schema tables plus prescriptive
guidance — that unblocks four of our roadmap features (F5 metal spots,
F6 geo vents, F7 features, F9 mapinfo editor) and resolves the F8
allyTeam gap. We will adopt the accepted recommendations as ADR-027
and use them to drive Phase 4 / Phase 5 implementation.

---

## Prompt (copy from here)

You are researching the **map metadata schema for Beyond All Reason (BAR)**,
an open-source RTS built on the **Recoil engine** (a fork of Spring RTS).
Your output is the reference document that an editor team will use to emit
correct, complete `mapinfo.lua` files plus any sibling Lua / data files
BAR expects inside a map's `.sd7` archive.

This research is needed because the editor team currently emits only a
*minimal* `mapinfo.lua` subset (just enough for BAR to boot a 1v1
skirmish). Adding metal spots, geo vents, features, multi-team layouts,
and a full mapinfo form-editor all require knowledge of fields and
conventions we are presently guessing at.

### Context: what the editor already does

- Single-binary Rust desktop app, produces a playable BAR `.sd7` archive
  via PyMapConv (CC0-1.0 sidecar) as the SMF/SMT compiler.
- The current `mapinfo.lua` emitter (a Rust string formatter, not an AST)
  outputs:

```lua
local mapinfo = {
    name        = "<name>",
    shortname   = "<name>",
    description = "",
    version     = "1",
    mapfile     = "maps/<name>.smf",
    modtype     = 3,
    depend      = { "Map Helper v1" },
    smf = {
        minheight    = <int>,
        maxheight    = <int>,
        smtFileName0 = "maps/<name>.smt",
    },
    teams = {
        [0] = { startPos = { x = ..., z = ... } },
        [1] = { startPos = { x = ..., z = ... } },
        -- ... up to N user-placed teams
    },
    lighting = {
        sundir = { 0.3, 1.0, -0.2 },
    },
}
return mapinfo
```

- This boots in BAR but produces a featureless, untextured map.
- The editor's data model expands incrementally: `start_positions` exists
  in the project struct now; future fields will be `metal_spots`,
  `geo_vents`, `features`, `splat_distribution`, and a `mapinfo_overrides`
  blob for everything the form-editor doesn't expose explicitly.
- The editor is *not* trying to support arbitrary Spring mods. BAR-only.
  Compatibility with the BAR engine + BAR mod (the "Beyond-All-Reason"
  game mod) is the bar to clear.

### What we need answered

**A. The full `mapinfo.lua` schema as the Recoil engine + BAR mod gadgets
consume it.**

Authoritative sources to hit:

1. **The Recoil engine source code** — repo
   `github.com/beyond-all-reason/spring`. Specifically:
   - `rts/Sim/Misc/SideParser.cpp` and related
   - `rts/Map/MapInfo.cpp` — the engine's mapinfo loader
   - `rts/Map/SMF/SMFReadMap.cpp` — SMF-specific fields
   - Any `Map/Lua*` files that surface mapinfo to gadgets
2. **The BAR mod** — repo `github.com/beyond-all-reason/Beyond-All-Reason`.
   Grep for `mapinfo` across:
   - `luarules/gadgets/`
   - `luaui/widgets/`
   - `luarules/configs/`
   - `LuaGaia/gadgets/`
   Identify every gadget that reads a mapinfo field. For each, name the
   gadget, the fields it reads, and what it does with them.
3. **Beherith's *Advanced SpringRTS Mapping Guide*** — the de-facto
   community reference. Hosted somewhere on `springrts.com` /
   `beyondallreason.info`. Identify and cite the canonical URL.
4. **The map-format reference gist** at
   `gist.github.com/Beherith/97cae4d300e675ca261e661fc58266d1` — already
   used by the editor team. Treat as a starting point, not a final
   answer.
5. **Sample mapinfo files from 5–10 popular published BAR maps.** Pick
   a representative spread: `Quicksilver`, `Glitters`, `Throne`,
   `Supreme Isthmus`, `gecko_isle_remake`, `All That Glitters Is Not
   Gold`, plus any FFA / 3-way maps you can find. Extract each
   `mapinfo.lua` (use `7z x <map>.sd7 mapinfo.lua` if you have a copy,
   otherwise inspect them via the BAR launcher's map cache or the
   `maps-metadata` repo). Diff field-by-field against the schema.

Output for this section: a **field table** with these columns —
`block.path`, `type`, `default if omitted`, `consumed by` (engine or
specific gadget by name+filename), `required for BAR?`, and a one-line
description. Don't drop any field — if `mapinfo.water.minColor` is read
somewhere, it goes in the table.

**B. `allyTeam` / `teams[]` schema (the F8 gap).**

The editor currently writes:

```lua
teams = {
    [0] = { startPos = { x = ..., z = ... } },
    [1] = { startPos = { x = ..., z = ... } },
}
```

This is wrong for any map that isn't 1v1. Specifically answer:

1. Does `teams[i]` carry an `allyTeam` field directly, or is allyteam
   membership encoded in a separate `allyTeams[]` table, or both?
2. What does a real 8v8 BAR map (e.g. `Quicksilver` for 8v8) emit? Get
   a real example.
3. What does a 3-way or 4-way FFA map emit (per-side starting positions
   grouped by allyteam)? Get a real example.
4. Are start-position spots inside one allyteam ordered (so the engine
   spawns players in a specific position-by-team-id mapping), or is the
   set unordered with the lobby picking from it? Inspect
   `rts/Game/GameSetup.cpp` or the BAR lobby's start-position handling.
5. Recommend a canonical layout for our editor's emitter that handles
   `1 ally x N teams`, `2 allies x N teams`, `3 allies x N teams`, and
   `K allies x N teams` cleanly.

**C. Metal spots (F5).**

There are two reasonable models. Which does BAR actually use?

1. **The metal map** — a `32 px / SMU` grayscale image where red-channel
   density encodes metal extraction value per pixel. Bundled in `.sd7`,
   compiled into the SMF.
2. **Lua-defined metal spots** — a `mapinfo.metal_spots = {}` array or a
   `LuaRules/configs/metal_spots.lua`-equivalent, where each spot is a
   discrete `{ x, z, value }` triple. Many BAR-mod metal-management
   widgets prefer this — they need precise spot centers, not a heatmap.

Investigate `luarules/gadgets/api_metal_spots.lua` (or similar in BAR),
and answer:

1. Does BAR currently use one model, the other, or both?
2. If both: which is canonical for new maps?
3. What's the exact Lua format if it's the per-spot model? Field names,
   coordinate space (elmos vs heightmap pixels vs metal map pixels),
   any required metadata (`metal`, `geo`, `extractRadius`)?
4. Are there widget gadgets that *require* the per-spot Lua format and
   fail or behave poorly without it? Name them.

Recommend which format(s) the editor should emit, and whether the
project model needs `metal_spots: Vec<MetalSpot>` only, or both that
plus a `metal_map: PathBuf` for the heatmap variant.

**D. Geo vents (F6).**

Geo vents power geothermal generators in BAR. Are they:

1. A subtype of "metal spot" with `geo = true`?
2. A separate `mapinfo.geo_spots = {}` or `LuaRules` config?
3. Encoded as map features (subtype of F7 features)?

Recommend the editor's data shape (`geo_vents: Vec<GeoVent>`) and the
emitter target. As above, cite the BAR gadget(s) that consume it.

**E. Features — trees, rocks, wreckage (F7).**

Three candidate locations the editor team has speculated about:

1. `mapinfo.lua features = {}` — inline in the manifest.
2. `LuaGaia/featuredefs.lua` — a separate Lua file inside the `.sd7`.
3. A custom gadget that reads `featurelist.lua` or similar at game-start.

Inspect BAR's `LuaGaia/`, `LuaRules/configs/`, and any
`features.lua`-style files in real maps. Determine:

1. Which file/format is canonical for BAR-mod features.
2. Whether "stock" features (BAR's default trees, rocks, wreckage)
   referenced by name are *zero-cost* in `.sd7` size (the mod owns the
   model files), or must be bundled.
3. The field schema per feature: `name`, `pos = { x, z }` or
   `pos = { x, y, z }`, `rot` (single angle vs Y-axis vs full matrix),
   `scale` (uniform vs per-axis), any allyteam ownership.
4. The full list of "stock" BAR features the editor's feature picker
   should default to. Source: the BAR mod's `featuredefs/` or
   equivalent.

Recommend our project model and emitter target. The SRS already flags
this distinction:

> **Default features (trees, generic rocks) are owned by the BAR mod
> and referenced by name** — zero `.sd7` payload, but the user's
> choices are limited to what the mod ships. **Map-custom features**
> would need their model + texture files bundled into the `.sd7`.

— confirm that this is still accurate and detail the bundling
mechanism for map-custom features.

**F. Other mapinfo blocks the editor will eventually expose (F9).**

For each of the following blocks, give us the field table (same columns
as section A) and one paragraph of "why a mapper would touch this":

1. `atmosphere` — fog colour, fog density, cloud density, sky colour.
2. `water` — water level, reflectivity, refractivity, colours, fresnel
   parameters. (Critical for any water-table map.)
3. `lighting` — beyond `sundir`: ambient, diffuse, specular colour for
   ground + units, shadow colour.
4. `terrainTypes` — per-type movement speeds, hardness, smoothness.
   Indices 0..15 (or similar); referenced from the type map.
5. `splats` — texScales, texMults, the 4 splat textures (these will be
   driven by the F4 splat-painting editor; the schema needs to match
   what PyMapConv writes into the SMT).
6. `resources` — if metal/geo are encoded here vs Lua gadgets.
7. `custom` — free-form table BAR widgets often read for per-map
   widget config. Sample what published BAR maps actually put here.
8. `smf` — beyond `minheight`/`maxheight`/`smtFileName0`: are there
   `voidWater`, `voidGround`, `smfheight`, `featuresFile` keys we
   should know about?
9. `gui` — minimap rotation, hint colours, anything else.

**G. Undocumented requirements / silent failure modes.**

The editor already knows three:

- `lighting.sundir` must exist or BAR's `luarules/gadgets/
  unit_sunfacing.lua` crashes (mapinfo defaults *should* cover this
  but the gadget reads without a nil check).
- `modtype = 3` is required for Chobby's map-browser filter
  (`gui_maplist_panel.lua`).
- `smtFileName0` must match the SMT filename inside the `.sd7` (the
  "pink map on rename" pitfall).

Find more. Specifically search BAR-mod gadgets for `mapinfo.<field>`
references without a `~=` nil check and list them. Each is a silent-
failure landmine our emitter must defend against.

### Required output structure

```markdown
# BAR mapinfo.lua + Gadget Schema Reference

## 1. Full mapinfo.lua field table
| Path | Type | Default | Consumed by | Required for BAR? | Description |
|---|---|---|---|---|---|
| `name` | string | — | engine | yes | ... |
| `smf.minheight` | int | 0 | engine | yes | ... |
| `lighting.sundir` | float[3] | — | `luarules/gadgets/unit_sunfacing.lua` | YES (gadget crashes without it) | ... |
| ... (every field) | | | | | |

## 2. allyTeam / teams[] schema
[Recommended Lua structure for 1v1, 8v8, 3-way FFA, with a worked
real-map example for each.]

## 3. Metal spots — F5 implementation guidance
[Recommend: metal map / Lua spots / both. Cite the gadget(s) that
consume each. Give the exact Lua field layout.]

## 4. Geo vents — F6 implementation guidance
[Same shape as section 3.]

## 5. Features — F7 implementation guidance
[Canonical file (`mapinfo.lua features`, `LuaGaia/featuredefs.lua`,
or other). Field schema per feature. Stock-feature list. Bundling
mechanism for map-custom features.]

## 6. Other mapinfo blocks — F9 schema
[Sub-tables for atmosphere, water, lighting, terrainTypes, splats,
resources, custom, smf-extras, gui. Each follows the same column
layout as section 1.]

## 7. Silent-failure landmines
[Bulleted list. Each item: `<field>` is read by `<gadget file path>`
at `<line>` without a nil check; if omitted, `<consequence>`.]

## 8. Recommended emitter output (Phase 4 / Phase 5)
[A complete `mapinfo.lua` for a hypothetical 8v8 map showing all of
the editor's new fields populated. This is the target our emitter
should reach by end of Phase 5.]
```

### Process constraints

- **Cite primary sources by file + line number** wherever a field's
  behaviour is claimed. Recoil engine `.cpp` files, BAR mod `.lua`
  files. Secondary sources (community wiki pages, forum posts) are OK
  as supporting context but every claim needs a primary citation.
- **One document, ≤ 2500 words excluding the field tables.** Tables can
  be as long as they need to be.
- **Recommend, don't catalog.** Don't say "the editor team should
  decide" — pick a specific Lua format and defend it. Where BAR has
  one canonical answer, give that. Where there's legitimate ambiguity
  (e.g. metal-map vs metal-spots), explain the tradeoff and recommend
  one.
- **Be explicit when something is undocumented.** If a field exists in
  the engine source but isn't used by any current BAR gadget, mark it
  as "engine-supported, BAR-unused — safe to skip."

---

## What we'll do with the output

1. Read the document end-to-end.
2. Accept / modify / reject section by section.
3. Commit the accepted schema as ADR-027 in `docs/DECISIONS.md`,
   superseding the minimal-emitter notes in ADR-013.
4. The `barme-pipeline::mapinfo` module grows to emit the new fields;
   `barme-core::Project` gets the corresponding model expansion
   (`metal_spots`, `geo_vents`, `features`, `mapinfo_overrides`,
   `ally_teams`). Each follows the `#[serde(default)]` forward-compat
   pattern already established in ADR-023.
5. F5 / F6 / F7 / F8-allyTeam / F9 implementation begins from this
   schema as the source of truth.
