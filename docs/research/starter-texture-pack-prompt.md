# Research prompt — BAR Map Editor starter texture pack

**Use:** Paste the section below verbatim into a fresh Claude deep-research
session. The session has no prior context on this codebase; everything it
needs is in the prompt.

**Expected output:** a draft ADR-shaped Markdown document recommending a
specific starter texture pack we can bundle (well, fetch-script) with the
editor. We will adopt it as `ADR-025` once accepted.

---

## Prompt (copy from here)

You are scoping the **starter texture pack** for an open-source desktop map
editor called *BAR Map Editor*. It is a single-binary Rust + egui + wgpu app
that produces playable `.sd7` map archives for **Beyond All Reason** (BAR), a
free RTS built on the Recoil engine (a fork of Spring RTS). PyMapConv (the
canonical SMF/SMT compiler, license CC0-1.0) runs as a bundled sidecar; the
editor authors heightmap + splat distribution + start positions, then
PyMapConv compiles the splat distribution against ≤ 4 splat tile textures
into the final SMT.

**The decision you are scoping:** *which 8–16 specific tile textures should
ship with the editor as a "starter palette" so a brand-new user can build a
visually believable map without sourcing any external assets?*

This is needed before we can implement F4 (splat painting), because the
splat brush UI needs to know what tile palette it's targeting.

### Constraints

1. **Licence** — Textures must be redistributable inside the editor's
   installer / AppImage / vendored fetch script with **zero attribution
   obligation** preferred (CC0-1.0). **CC-BY 4.0 is acceptable if and only
   if a single bundled `CREDITS.md` line covers the whole pack** — we will
   not chase per-texture attribution UX. GPL-style / share-alike / NC
   licences are disqualifying because the user's `.sd7` outputs would
   inherit obligations.

2. **Format & resolution** — PyMapConv's splat tile texture input is
   typically 1024 × 1024 BMP or PNG (verify this — the canonical reference
   is Beherith's *Advanced SpringRTS Mapping Guide* and the PyMapConv
   `mapconv.py` source at `github.com/Beherith/springrts_smf_compiler`).
   The editor wants the *source* assets in a higher-res lossless form
   (PNG-8 1024² or 2048²) so the eventual `barme-pipeline` can BC1-compress
   on the fly via Compressonator (already vendored). If PyMapConv expects
   specific filename conventions / colour spaces (sRGB vs linear), note them.

3. **Tiling** — Every texture **must tile seamlessly**. We are not going to
   author edge-blending; a starter pack with visible repeats is a non-
   starter. Prefer textures explicitly authored for seamless tiling
   (ambientCG, Poly Haven, Texture Haven all flag this).

4. **File-size budget** — Total starter pack ≤ **50 MB on disk** (the
   fetch script downloads once into `tools/textures/`, gitignored). If
   JPEG-encoded sources let us hit 8 MB at perceptually-equivalent quality,
   prefer that for diffuse-only textures; PNG for anything where chroma
   subsampling would corrupt (we don't author the splat distribution from
   these — they're the *target* tiles — but pixel-exact tiling matters at
   close zoom).

5. **Biome coverage** — A starter palette should let a user build the four
   most-common BAR-map archetypes without external sourcing. **Survey what
   archetypes actually ship.** Inspect the BAR maps catalogue
   (`github.com/beyond-all-reason/Beyond-All-Reason`,
   `github.com/beyond-all-reason/maps-metadata`, the in-game map list, and
   the Chobby Map Browser). Identify the dominant biomes by playcount /
   tournament use. As a starting hypothesis (verify or refute):
   - Earth-temperate (grass meadow, forest floor, dirt, rocky outcrop)
   - Arid / desert (sand, dry rock, dusty hardpan)
   - Snow / alpine (snow, ice, cold rock, frozen sand)
   - Alien / volcanic / industrial (BAR has a notable alien-orange and
     volcanic-black aesthetic — `gecko_isle_remake`, `Throne`, `Quicksilver`
     reference)

   Then propose **one starter palette of 8–16 textures** that spans the
   four archetypes without redundancy. Each entry should be a single tile
   texture (e.g. one "grass meadow"), not a multi-variant set.

6. **Source provenance** — Concrete URLs only. Each entry should cite:
   - Source site (ambientCG / Poly Haven / OpenGameArt / etc.)
   - Direct download URL
   - Author (if CC-BY) or "Public Domain" marker
   - File size + resolution at the chosen source quality
   - Licence URL + SPDX identifier

### Research questions

Answer each in your output (cite sources for each — primary sources, not
secondary blog posts; if a fact is unverifiable in 2-3 searches, mark it
explicitly as a hypothesis that the implementation phase will need to test).

**A. PyMapConv / SMT splat behaviour.**
1. What resolution does PyMapConv consume splat tile textures at, by default?
   Is it user-overridable per invocation?
2. Does PyMapConv synthesise the normal map from the diffuse, or do we need
   to bundle separate normal maps? If separate: are they optional?
3. What colour space does PyMapConv expect (sRGB, linear)? Does it care?
4. Are there filename / channel-order conventions? (e.g. does PyMapConv
   expect `splat0.bmp ... splat3.bmp` in a specific order? Or does mapinfo
   `splats = { tex1 = "...", tex2 = "..." }` route them by string name?)
5. What is the maximum splat texture count? The SRS says ≤ 4 (4 RGBA
   channels of the distribution map) — confirm against the engine's
   `Map/SMF/SMFRenderState.cpp` or equivalent.

**B. BAR map biome survey.**
1. Inspect `beyond-all-reason/Beyond-All-Reason` and
   `beyond-all-reason/maps-metadata` — what biomes are most common across
   the official-list maps?
2. Are there community-recognised "BAR style" texture references? (Some
   maps share an aesthetic — Throne, Glitters, Quicksilver, Supreme Isthmus
   — is there a common source for their textures?)
3. Sample 3–5 popular maps (e.g. Quicksilver, Glitters, Throne, gecko_isle,
   All That Glitters Is Not Gold). For each, extract `mapinfo.lua` from the
   `.sd7` and list its splat texture references. Is there overlap?

**C. Existing precedents.**
1. **JandoDev/bar-editor** (`github.com/Jandodev/bar-editor`) — does it
   bundle textures? If so, which ones, and what licence?
2. **`tebeer/BARMapEdit`** (Unity-based, stalled) — for completeness, did
   it have a starter pack?
3. The Spring/Recoil community used to share a "Spring Map Texture Pack" —
   does one currently exist? Under what licence?

**D. License-clean source catalogues.**
1. **ambientCG** (`ambientcg.com`) — CC0-1.0 catalogue. Tile-seamless
   guaranteed. Survey their "Ground" / "Rocks" / "Snow" / "Sand" / "Lava"
   categories for the 8–16 best fits.
2. **Poly Haven** (`polyhaven.com/textures`) — CC0-1.0 since 2021 (verify
   the licence is still CC0 for newly-uploaded textures). Their "outdoor"
   library is high-quality but smaller than ambientCG.
3. **OpenGameArt** (`opengameart.org`) — mixed licences. Avoid unless a
   specific texture is CC0 and unavailable elsewhere.
4. **Texture Haven** — now folded into Poly Haven; check whether the
   archive URLs at `texturehaven.com` still resolve or 301 to Poly Haven.

### Deliverable

A draft ADR in this exact shape (we will commit it as
`docs/DECISIONS.md` § ADR-025 once reviewed):

```markdown
## ADR-025 — Starter texture pack

**Status:** Proposed (research) — 2026-05-DD
**Context:** [why this decision is needed now; reference F4; reference the
              survey findings from research-question section B]
**Alternatives:** [bundled pack vs in-app downloader vs user-import-only
                   vs forking an existing community pack — one line each]
**Consequence:**
- 8–16 textures bundled via `scripts/fetch-textures.sh` into
  `tools/textures/`. Total ~XX MB. Licence audit table below.
- Splat brush UI defaults to this palette; user-import (F23 / Phase 6) is
  the polish path.
- [any PyMapConv plumbing implications — e.g. "the pipeline must convert
  source PNGs to BMP at load time" — based on research-question A]

### Bundled textures

| Slot | Name | Source | Direct URL | Licence | Size (source) | Biome |
|---|---|---|---|---|---|---|
| 0 | grass-meadow-01 | ambientCG | ... | CC0-1.0 | 4.2 MB / 2048² PNG | temperate |
| 1 | ... | ... | ... | ... | ... | ... |

[8–16 rows total]

### Excluded but considered

[1–3 textures you researched and rejected, with one-line reason]

### Open questions for implementation

[anything the research couldn't definitively answer — e.g. "the channel-
 ordering convention for PyMapConv splats needs verification by building
 a test map; ambientCG metadata is correct in principle but two-sample
 verification on a real BAR install before locking the palette."]
```

### Constraints on your research process

- **Cite primary sources.** PyMapConv questions → its repo / source. BAR map
  biome questions → the maps-metadata repo or extracted `mapinfo.lua` files.
  Texture licence questions → the catalogue's own licence page (ambientCG's
  changes their boilerplate; verify per-texture, not per-site).
- **One ADR, not a treatise.** ≤ 1500 words excluding the bundled-texture
  table. Crisp recommendations, not exhaustive surveys. Include the bundled-
  texture table even if it's tentative.
- **Skip aesthetic critique.** Pick textures that work; don't get into
  "this grass is overly saturated" — that's iteration-time feedback.

---

## What we'll do with the output

1. Review the draft ADR against this codebase's context (any PyMapConv
   findings get reconciled with ADR-013 / ADR-014).
2. Commit it as `docs/DECISIONS.md` § ADR-025.
3. Write `scripts/fetch-textures.sh` matching the pattern of
   `scripts/fetch-pymapconv.sh` (sha256-pinned, gitignored target dir).
4. Implementation phase begins (F4 commit 1).
