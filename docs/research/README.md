# Research

Long-form research that informs roadmap features. Lives outside
`docs/DECISIONS.md` (ADRs) because:

- ADRs are short, prescriptive, and committed *after* a decision lands.
- Research is exploratory, may span thousands of words, often comes from
  an offline deep-research session, and exists *before* a decision is
  made. Once accepted, it gets distilled into an ADR.

## Layout — one folder per topic

Each topic owns a folder with three files:

| File | Purpose |
|---|---|
| `PROMPT.md` | The brief we hand to a deep-research session (a fresh Claude with no project context). Self-contained — every constraint and source the session needs is in here. |
| `FINDINGS.md` | The session's output, copied in verbatim. Treat as read-only once landed; corrections go in `NOTES.md` so the original is auditable. |
| `NOTES.md` | Our review: accept / modify / reject per section, the ADR number(s) it became, follow-up questions, anything the findings missed. |

Some topics also grow a `sources/` subfolder with downloaded reference
material (PDFs, screenshots, extracted code, sample mapinfo files).

## Status

| Topic | Status | Drives | Becomes ADR |
|---|---|---|---|
| `textures/` | Prompt drafted, session pending | F4 splat painting | ADR-025 |
| `ui/` | Prompt drafted, session pending | UX overhaul across Phase 3 | ADR-028 (anticipated) |
| `mapinfo/` | Prompt drafted, session pending | F5, F6, F7, F8 allyTeam gap, F9 | ADR-027 |

Update this table when a topic transitions:
- *Prompt drafted* → *Session pending*: we've handed PROMPT.md off
- *Session pending* → *Findings landed*: FINDINGS.md exists, awaiting review
- *Findings landed* → *Reviewed*: NOTES.md has accept/reject decisions
- *Reviewed* → *Adopted (ADR-N)*: the ADR is committed in `docs/DECISIONS.md`

## How to run a research topic

1. Read the topic's `PROMPT.md`.
2. Paste the section marked "Prompt (copy from here)" into a fresh
   Claude session (deep research mode preferred).
3. Save the session's output into `FINDINGS.md`. Don't edit it — the
   raw output is the audit trail.
4. Read FINDINGS, write `NOTES.md` with our review (section-by-section
   accept / modify / reject, plus ADR number once committed).
5. Distill accepted recommendations into an ADR. Update this README's
   status table.

## When to start a new topic

When you find yourself guessing about something *outside* the editor's
own architecture (engine internals, external library choice, asset
licensing, community conventions, UX patterns from other tools), that's
a research topic. When you're guessing about something *inside* our
architecture (whether to put a method on `Heightmap` or in
`brushes::mod`), that's an ADR. Research feeds ADRs; ADRs don't feed
research.
