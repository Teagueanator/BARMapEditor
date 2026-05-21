# Sculpt

Heightmap brush. The headline tool — terrain is the foundation
for everything else.

LMB drags stamps the active brush. Pick `Raise`, `Lower`, or
`Smooth` from the Inspector. Brush radius (`8..4096` elmos) and
strength (`0..1`) tune the per-stamp behaviour; falloff is a
fixed ease-out for Sprint 19 (per-brush curves are a later
sprint). Symmetry replicates strokes across the selected axis.

Undo (`Ctrl+Z`) reverts the most recent stroke. The undo ring is
capped at 100 MB; long brush strokes inside a single LMB-hold
share one snapshot (ADR-041) so the cap is rarely reached.

## Tips

- 1 SMU = 512 elmos = 65 heightmap pixels. A 256-elmo brush is
  about half an SMU wide. Plan brush size against feature size.
- Smooth before bake — small bumps look great in the editor and
  read as jagged seams once DNTS runs over them in BAR.
- Symmetry is non-negotiable for competitive maps. Toggle it from
  the top bar; the fold knob picks N for rotational maps.
- Procgen (`G`) is faster for the base elevation; finish in
  Sculpt with the fine details.
