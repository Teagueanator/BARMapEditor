# Metal spots

Place mex extraction points. BAR's `gui_metalspots` widget reads
these from `mapconfig/map_metal_layout.lua` (engine convention)
and overrides the engine metalmap entirely — meaning the SMF
metalmap must be all-zero or the gadget bails and your Lua spots
are ignored (PITFALL §13). The build pipeline ships an all-zero
metalmap PNG whenever there are authored spots.

LMB places a new spot with the standard `metal = 2.0` yield.
LMB-drag moves spots; RMB deletes. Symmetry replicates per
mirror.

## Yields

BAR convention:

- `0.5` — perimeter / risky positions
- `2.0` — standard "fat mex"
- `4.0–5.2` — central / strategic positions

Per-spot value is freely settable via the Inspector `DragValue`.

## Extractor radius

The Inspector `extractor_radius` slider sets how close two
neighbouring spots can be before BAR clusters them in the F4
metal view. Default **80** matches BAR; the engine default
**500** breaks mex-snap and yields confusing F4 placements
(PITFALL §6). A chip flags any value other than 80 so you can't
miss it.
