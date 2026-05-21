# PITFALL §25 — `LuaGaia/Gadgets/` needs a map-bundled bootstrap

Shipping a gadget at `LuaGaia/Gadgets/Foo.lua` does **nothing**
on its own. The engine only scans that directory when the map
carries a `LuaGaia/main.lua` that `VFS.Include`s the
engine-provided `LuaGadgets/gadgets.lua` handler.

`springcontent.sdz` (verified against recoil 2026.06.04)
provides the handler but **not** a fallback bootstrap, so the
map MUST ship its own.

## Rule

Every `.sd7` ships:

```
LuaGaia/main.lua    -- synced bootstrap
LuaGaia/draw.lua    -- unsynced-draw bootstrap
```

with these two-line bodies:

`main.lua`:

```lua
if AllowUnsafeChanges then AllowUnsafeChanges("USE AT YOUR OWN PERIL") end
VFS.Include("LuaGadgets/gadgets.lua",nil, VFS.BASE)
```

`draw.lua`:

```lua
VFS.Include("LuaGadgets/gadgets.lua",nil, VFS.BASE)
```

`build_sd7` stages both files into every `.sd7`. They are
vendored at `crates/barme-pipeline/assets/luagaia_{main,draw}.lua`
and exposed as `featureplacer::LUAGAIA_{MAIN,DRAW}_SOURCE`.

## How this manifests

If you ship a custom gadget without the bootstrap pair:

1. The `.sd7` looks correct on disk.
2. BAR loads the map without error.
3. Your gadget's `gadgetHandler:RegisterCMDID`,
   `gadget:Initialize`, etc., are never called.
4. Features the gadget would have placed don't spawn.

This is exactly what happened to the editor's Sprint 11
geo-vents work — the Springboard featureplacer trio was staged
correctly, but the gadget was never loaded because the
bootstrap was missing.

## Pre-merge gate

Any future "ship a gadget in `LuaGaia/Gadgets/`" change must
extract a real BAR map and diff our SD7 against it at the
`LuaGaia/` level.
