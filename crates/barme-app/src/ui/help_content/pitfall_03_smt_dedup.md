# PITFALL §3 — SMT tile dedup

The SMT format hash-deduplicates 32×32 tiles. Naïve output
produces SMTs roughly 4× larger than tuned output. PyMapConv
has the deduplicator; if we ever fork it, port the hash table
verbatim.

## Rule

Don't reimplement. If a fork is forced, copy the hash table
implementation byte-for-byte and reference the upstream SHA.

## Why this matters

A 16×16 map's diffuse is 8192² (256 MB RGBA). Sliced into 32×32
tiles, that's 65 536 tiles. Many of them are visually identical
(grass, snow, sand, repetitive patches). The deduplicator
collapses these to a single hashed entry; the SMF's tile-index
array references the pool.

A 16×16 SMT typically lands at 40–80 MB compressed. Without
dedup it'd be 320 MB+. Multiply by every map a user installs and
the disk savings add up fast.

## What you might see

If a build produces a much larger `.smt` than expected, the
deduplicator may not have engaged. Check the PyMapConv log for
the per-tile dedupe statistics — typically "Saved N tiles" on
success.
