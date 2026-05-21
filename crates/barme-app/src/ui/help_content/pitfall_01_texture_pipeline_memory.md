# PITFALL §1 — Texture pipeline memory

A 16×16 BAR map = 8192² diffuse (256 MB RGBA) + 8192² normal
(256 MB) + 4096² splat distribution (64 MB). A naive snapshot-undo
on these blows past 4 GB.

## Rule

Edit buffers are tiled 256×256 chunks, copy-on-write, disk-backed
LRU. Undo deltas are per dirty tile, never full snapshots
(ADR-033).

## Why this matters

Sprint 17's mask architecture uses `MaskTiles` (tiled COW) for
the same reason; brush strokes scoped to a single bounding rect
upload only the touched tiles. The Sprint 14 / ADR-033 work
collapsed a single radius-1024 sculpt stroke from ~244 MB per
entry to ≤ ~2 MB by hoisting the stroke-scope deduplication into
`History`. The 100 MB ring is now actually a 100 MB ring.

## What you might see

If the editor starts spending all its time paging or your laptop
fan kicks in during sculpting, file an issue with the project
size + how many tools you've used in the session. The undo path
is the most likely culprit.
