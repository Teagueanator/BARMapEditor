# Build pipeline

Build + Install runs a six-stage pipeline:

1. **PrepareStaging** — copies the heightmap, type/metal PNGs,
   splat distribution, baked DNTS DDSes, mapinfo.lua, the
   Springboard featureplacer trio, and the LuaGaia bootstrap into
   a temp directory.
2. **PyMapConv** — invokes the vendored PyMapConv (CC0-1.0) with
   AMD Compressonator to compile `.smf` + `.smt`. PyMapConv is a
   sidecar, not a re-implementation — see [ADR-002 / ADR-011].
3. **EmitMapInfo** — serialises the mapinfo.lua AST. Three
   independent gates (engine, Chobby, mod gadgets) all need to
   pass; see PITFALL §A through §C.
4. **PackageSd7** — 7z packs with `-ms=off` (PITFALL §9 — solid
   archives are silently rejected by SpringFiles).
5. **Install** — copies the `.sd7` into BAR's user maps directory
   (`~/.local/state/Beyond All Reason/maps/` on Linux).
6. **Done** — toast confirms the install path; clicking opens
   the directory in your file manager.

The build runs on a worker thread; the UI stays interactive.
Click View log to follow PyMapConv's stdout as it arrives.

## Common failures

- **`FileNotFoundError: temp/thread0/temp0.dds`** — PyMapConv
  v0.6.3 has a known read-back bug on Linux when `numthreads > 1`.
  The driver always passes `-q 1`; if you see this anyway, file
  an issue with the log attached.
- **PyMapConv exits 1 but artifacts present** — known issue on
  Linux ("All Done!" followed by exit 1 due to a Qt event-loop
  quirk). The driver treats artifact-presence as the success
  contract; the log will show this as a warning, not a failure.
- **Pink map in BAR** — the `.smt` was renamed without updating
  `mapinfo.smf.smtFileName0`. Re-build; the editor regenerates
  the matching mapinfo entry every save.
- **Game hangs at "waiting for players"** — a mod gadget crashed
  on a missing mapinfo subtable. The lint pass should have
  caught this; if it didn't, attach the log and file an issue.

See [Pitfalls](#pitfalls) for the silent-failure catalogue.
