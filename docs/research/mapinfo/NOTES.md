# Notes — BAR mapinfo.lua + gadget schema

Our review of `FINDINGS.md`. Section-by-section accept / modify / reject,
plus any context the findings missed.

---

(write up after FINDINGS.md lands)

## Adoption

Once accepted, the schema becomes **ADR-027** in `docs/DECISIONS.md`,
superseding the minimal-emitter notes in ADR-013. The schema then
drives implementation of:

- **F5** Metal-spot placement (Phase 4)
- **F6** Geo-vent placement (Phase 4)
- **F7** Feature placement (Phase 4)
- **F8 allyTeam encoding** (cleanup — gap flagged 2026-05-18)
- **F9** mapinfo.lua editor (Phase 5)

Update `../README.md` status table on adoption.
