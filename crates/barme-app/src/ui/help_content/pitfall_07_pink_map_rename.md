# PITFALL §7 — Pink map on rename

Modern Recoil reads the SMT filename from `mapinfo.smf.smtFileName0`
rather than the historical hardcoded slot in the SMF binary. If
you rename the SMT but forget to rewrite the mapinfo entry, BAR
falls back to its missing-texture pink debug colour.

## Rule

Rename is a single atomic operation that rewrites BOTH the SMT
filename and the matching `mapinfo.lua` entry. The build pipeline
regenerates `smtFileName0` to match the staged SMT every time, so
in practice you'll only hit this if you hand-edit the `.sd7`.

## Diagnosing

If your map is pink in BAR but renders fine in the editor:

1. Unzip the `.sd7`. The `maps/` directory should have a `.smt`
   whose filename matches `mapinfo.smf.smtFileName0`.
2. The wrong stem will show up as either a missing file (BAR
   logs "smtFileName0 not found in archive") or a mismatched
   stem (BAR loads the wrong tile pool).
3. Re-running Build from the editor regenerates both halves and
   fixes the link.

## Why pink

The `0xff00ff` magenta is the engine's "I tried to look up a
texture and it isn't there" debug colour. Other Spring-era
games use it for the same purpose.
