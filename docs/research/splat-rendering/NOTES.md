# Notes — Recoil terrain shader + splat / DNTS composite math

Our review of `claude-research-findings.md` and
`gemini-research-findings.md`. Section-by-section accept / modify /
reject, plus any context the findings missed.

---

(write up after at least one FINDINGS file lands; ideally both, for
side-by-side cross-check)

## Adoption

Once accepted, the formula becomes **ADR-035** (or next available ADR
number when the research lands) in `docs/DECISIONS.md`. The ADR then
drives:

- **Sprint 9 / D4** — wgpu fragment shader for the splat composite.
- **Sprint 9 / D5** — splat tool UI uses the accepted texScales /
  texMults default ranges.
- **Sprint 11 / D6** — pipeline emission of `resources.splatDetailNormalTex`
  + `splats.texScales` / `texMults`.
- **Sprint 13 / C8** — linter rule for the `splatDetailNormalTex` ↔
  `specularTex` dependency, with the conditional confirmed against
  current Recoil source rather than carried as folklore.

Update `../README.md` status table on adoption.
