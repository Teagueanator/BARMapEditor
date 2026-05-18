# Pitfalls

The ten silent failure modes from SRS §2.1, restated as engineering rules with
the test or invariant that catches each one.

## 1. Texture pipeline memory

A 16×16 map = 8192² diffuse (256 MB RGBA) + 8192² normal (256 MB) + 4096²
splat distribution (64 MB). Snapshot-undo of full images blows past 4 GB.

**Rule:** Edit buffers are tiled 256×256 chunks, copy-on-write, disk-backed
LRU. Undo deltas are *per dirty tile*, never full snapshots.

## 2. DXT1 is slow and lossy

Quality-tuned compression of a 16×16 takes 1–10 min. SMT mandates DXT1 — BC7
is not an option.

**Rule:** In-process BC1 (texpresso/bcdec/ISPC) for live preview. PyMapConv +
Compressonator for final-quality `.smt`. (Note: PyMapConv switched off
nvdxt.exe to Compressonator some time before May 2026 — no Wine needed on
Linux.) **CompressonatorCLI is invoked by name** (no path override in
upstream `src/pymapconv.py`); we vendor it under `tools/compressonator/`
and prepend that dir to `PATH` for the subprocess (ADR-014).

## 3. SMT tile dedup

Naïve SMT output is ~4× larger than tuned output. PyMapConv has the hash
deduplicator; if we ever fork, port verbatim.

**Rule:** Don't reimplement. If a fork is forced, copy the hash table
implementation byte-for-byte and reference the upstream SHA.

## 4. Heightmap edge constraint — `64·N + 1`

The #1 silent corruption. PyMapConv warns + resizes; user sees wrong terrain.

**Rule:** `MapSize::heightmap_dims()` is the only place dims are computed.
Any image import path rejects (with explicit error) — never silently crops or
pads. Unit test in `crates/barme-core/src/map_size.rs` pins the math.

## 5. Coordinate sign flips

Spring: Y-up, left-handed. Heightmap pixel `(x, y)` → world `(x·8, h, y·8)`.
Lua features use `{x, z, rot}` in elmos. The legacy `-i / --invert` flag
exists because of historical row-order confusion.

**Rule:** A single internal coordinate convention, documented in
`docs/ARCHITECTURE.md`. All converters live in one module. No ad-hoc flips.

## 6. `mapinfo.lua` silent dependencies

- `splatDetailNormalTex` requires `specularTex` (silently disables otherwise)
- `voidWater` requires unsetting `water.planeColor`
- Missing/renamed `smtFileName0` → the pink map
- `fogStart == fogEnd` breaks the ground-grid renderer

**Rule:** Linter pass before every save in `barme-mapinfo`. Each of these is
a named lint with a test fixture.

## 7. Pink-map trap on rename

Modern Recoil reads `mapinfo.smf.smtFileName0`. The SMT filename is no longer
hardcoded into the SMF, but if `mapinfo.lua` isn't rewritten on rename →
pink.

**Rule:** Rename is a single atomic operation that rewrites BOTH the SMT
filename and the matching `mapinfo.lua` entry.

## 8. DNTS + water + LOS animated-snow bug

`minHeight < 0` + DNTS + a Lua widget that touches LOS → TV-snow artifact
(Beherith, springrts forum t=35202).

**Rule:** Warn (don't block) when DNTS is enabled on a map with
`minHeight < 0`. Surface in the linter as a yellow warning.

## 9. `.sd7` solidity

7-Zip *solid* archives are silently rejected by SpringFiles indexing.

**Rule:** Packager invokes `7z` with `-ms=off`. Integration test opens the
output and asserts `IsSolid == false`.

## 10. PyMapConv license / redistribution

SRS flagged this as unresolved. **As of May 2026, PyMapConv ships with a
CC0-1.0 LICENSE** — redistribution is unrestricted. This pitfall is
historically interesting but no longer a blocker; we still verify the
LICENSE file is present in each vendored release.

**Rule:** The vendor script asserts a LICENSE file exists in the downloaded
PyMapConv archive and that its SPDX identifier is permissive.

---

## Bonus (not numbered but cited in SRS §2.1)

- **3D preview ≠ in-game.** Document the gap up front; do not pretend WYSIWYG.
- **Decompilation fidelity.** Round-trip loses diffuse precision (DXT1).
  Heightmap, metal, type, mapinfo are exact. Reuse PyMapConv's decompile path.
- **GPU brush latency.** Heightmap lives on the GPU as an R16 storage texture,
  edited by compute shaders. Read-back to CPU only on save.

## PyMapConv v0.6.3 Linux runtime quirks (found in Stage 0, ADR-014)

- **Always pass `-q 1` on Linux.** Default `numthreads=4` triggers an
  upstream read-back bug: tile compression writes flat into
  `temp/temp{i}.dds`, but the read-back loop checks `numthreads > 1`
  and tries `temp/thread{n}/temp{i}.dds` (the Windows multi-thread
  layout that Linux never creates). Crash:
  `FileNotFoundError: temp/thread0/temp0.dds`. Source: v0.6.3
  `src/pymapconv.py` lines 960–986.
  **Rule:** the driver passes `-q 1` unconditionally on Linux.

- **Trust artifact presence, not exit code.** PyMapConv exits with
  status 1 on Linux even after `All Done!` — the bundled Qt event loop
  closes "abnormally" when no display is held open. The contract is
  what's on disk (`.smf` + `.smt`).
  **Rule:** treat artifact-presence as success and log non-zero exit
  at `warn`. Only fail when artifacts are missing AND exit was
  non-zero.

## BAR Chobby + mod-gadget mapinfo expectations (found in Stage 0, goal #7)

The "engine-documented minimum" mapinfo is **not** the "real-world
minimum to play a BAR map." Three discrete gates a `.sd7` must clear,
each with different requirements:

### A. Engine scanner — extremely lax

`name`, `smf.smtFileName0`, and `teams[*].startPos` are the only
strictly required fields per the BAR map archive format reference (gist:
`burnhamrobertp/97cae4d300e675ca261e661fc58266d1`, "bare-minimum viable
map"). Everything else has engine defaults.

### B. Chobby map browser — filters on certification + modtype

`gui_maplist_panel.lua` in `beyond-all-reason/BYAR-Chobby` filters maps
by `info.modtype == 3` AND by a hardcoded "certified maps" list
shipped inside Chobby. Maps not in the list get
`certification = "Unofficial"`.

**Rule:** unofficial maps **only appear in Skirmish / singleplayer
lobbies**. Multiplayer lobbies hide them entirely. The
`[Chobby] Warning: GetMinimapImage not found for, <name>` warning is
benign — `api_map_handler.lua` auto-extracts the minimap from the SMF on
first scan; the warning fires once before extraction completes.

### C. BAR mod gadgets — fragile reads with no nil guards

BAR's mod-side Lua gadgets read mapinfo fields directly without
nil-checking the subtables. The first one we hit:
`luarules/gadgets/unit_sunfacing.lua` line 44:
```lua
sundir = mapinfo.lighting.sundir
```
With no `lighting` subtable in mapinfo, this throws
`attempt to index field 'lighting' (a nil value)` during the LuaRules
load phase — game appears to start but waiting-for-players hangs forever
because the synced state never completes.

**Rule:** the emitter must include the conventional subtables (at
minimum `lighting = { sundir = {…} }`, likely also `atmosphere`,
`water`, `terrainTypes`) even though the *engine* has defaults for
them. The list of subtables to include grows as we discover more
gadgets with this pattern — when a new crash surfaces, add the
required field with a sensible default and a regression test in
`barme-pipeline::mapinfo::tests`.

Reference: a complete in-the-wild example is
`scratch/bar-maps/extracted/titanduel/mapinfo.lua` (gitignored — copy
from `~/.local/state/Beyond All Reason/maps/titanduel_v3.sd7` to
inspect).
