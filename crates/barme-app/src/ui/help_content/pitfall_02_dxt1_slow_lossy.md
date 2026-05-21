# PITFALL §2 — DXT1 compression is slow and lossy

The SMT tile format mandates DXT1 (`compressionType = 1,
tileSize = 32`). BC7 is not an option. Quality-tuned compression
of a 16×16 map takes 1–10 minutes; tighter settings make it
noticeably better.

## Rule

In-process BC1 (texpresso / bcdec / ISPC) for live preview;
PyMapConv + AMD Compressonator for final-quality `.smt`.

## What changed

Pre-2026-05 PyMapConv shelled out to NVIDIA's `nvdxt.exe` —
a Windows binary that required Wine on Linux. Sprint 0 / ADR-004
shipped the switch to **AMD Compressonator** (native Linux,
open-source, MIT). No Wine dependency.

Compressonator is invoked by name (no path override in upstream
`src/pymapconv.py`). The editor vendors it under
`tools/compressonator/` (ADR-014) and prepends that directory
to `PATH` for the PyMapConv subprocess.

## Preview vs final

The editor's live preview uses a much faster BC1 encoder — the
goal there is sub-100 ms feedback while painting. The final
`.smt` uses Compressonator's higher-quality settings. The visual
gap is small enough that "what you see is what you get" mostly
holds; the renderer-parity arc (Sprints 23+) tightens it.

## Decompression

Round-tripping an existing `.sd7` loses diffuse precision (the
DXT1 → RGBA decompress path is lossy). Heightmap, metal, type,
and mapinfo round-trip exactly. Reuse PyMapConv's decompile
path; don't reinvent it.
