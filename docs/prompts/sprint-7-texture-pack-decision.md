# Sprint 7 — Starter texture pack decision + fetch script (D1)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 7** from `devlog/stage-1-mvp/phase-3-plan.md` § "Order of
attack." You ship one item — **D1**, the starter texture pack decision +
fetch script. It's small, but solo because (a) it's an ADR-heavy
decision-and-license-audit item that warrants careful sourcing, and (b) it
gates the entire D-stream (D2+ all assume the palette is locked).

**Independence note:** D1 is **independent of Sprints 5 and 6**. It can run
in parallel with them — if you're picking this up while another session is
on Sprint 5 or 6, that's expected. Don't block on those.

**Prerequisites:** Sprints 1–4 (A1–A4, B1–B5, C1) should be ticked.
Sprints 5 and 6 *may or may not be done* — D1 doesn't touch the runtime,
just adds fetch tooling, an ADR, and a CREDITS file. No code changes to
`crates/`.

**UX context (ADR-035, already shipped):** the editor already has a
**Splat paint** tool tile in the left tool strip (keyboard `T`) and a
scaffolding inspector at `crates/barme-app/src/main.rs::inspector_splat`
that renders a 4-row RGBA layer list, RGBA channel chips, brush-mode
buttons (Paint / Erase / Smear), and radius / strength / spacing
sliders. The scaffolding state lives on `App::splat_state` with four
seeded layers (Grass / Rock / Sand / Snow). D1's palette structure
must feed that inspector — D5 will swap the in-memory state for one
driven by your fetched `tools/textures/<NN-slot>/meta.toml` files.
Slot naming (`NN-kebab-name`) and `meta.toml` schema thus become a
durable contract; the inspector will display whatever the registry
reads.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules. Note "Things
   deliberately NOT in this repo" — `tools/` is gitignored; vendored
   binaries are fetched, not committed.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (mapinfo splat fields),
   §2.1 (texture-pipeline pitfalls), §3.2 F4 / F23.
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — non-negotiable rules.
4. `/home/teague/code/BARMapEditor/devlog/README.md`.
5. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/goals.md`.
6. `/home/teague/code/BARMapEditor/devlog/stage-1-mvp/phase-3-plan.md` —
   read **D1 in full**, plus D2 / D3 / D6 so you understand what your
   palette structure needs to support.
7. `/home/teague/code/BARMapEditor/docs/research/textures/claude-findings-from-research.md`
   — Claude's deep research.
8. `/home/teague/code/BARMapEditor/docs/research/textures/Gemini BAR Editor Texture Pack Scoping.md`
   — Gemini's deep research. **Adopt Gemini's 16-slot palette verbatim**
   (no duplicate IDs, coherent biome coverage, primary-source-justified
   industrial set). **Adopt Gemini's bundle-the-normal-map stance** over
   Claude's synthesise-from-luminance — see phase-3-plan.md D1 for the
   rationale.
9. **`/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`**
   §7 — the corrected splat-rendering math. This affects D2's bake
   pipeline (Sprint 8) but knowing it now informs the palette choice:
   the engine decodes ALL of `splatDetailNormalTex.rgba` as signed
   (`* 2 - 1`), so the alpha channel either ships at solid `0xFF`
   (when `splatDetailNormalDiffuseAlpha = false`) or as a signed
   high-pass diffuse offset (when `true`). Texture pack baseline is
   `false` — A=255 in every DDS.
10. `/home/teague/code/BARMapEditor/scripts/fetch-pymapconv.sh` and
    `/home/teague/code/BARMapEditor/scripts/fetch-compressonator.sh` —
    reference patterns. Your `fetch-textures.sh` mirrors them: sha256-
    pinned, idempotent, downloads into a gitignored `tools/` subtree.
11. ADRs 003 (PyMapConv license — CC0; reference for license hygiene),
    011 (fetch-script convention), 014 (Compressonator script pattern).

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-texture-pack
./devlog/log.sh log stage-1-texture-pack "starting"
```

Fill `stage-1-texture-pack/`:
- `goals.md` — from D1's Scope + Success criteria.
- `theories.md` — from D1's Hypothesis (none stated; this is a decision-
  driven item, not a falsifiable claim).
- `notes.md` — palette table, license audit results, fetch-script design
  notes.
- `logs/<timestamp>__<title>.md` — session logs.

## Step 3 — Scope

Ship D1. One implementation commit (fetch script + ADR + CREDITS) plus a
rollup commit.

**D1 deliverable:**

- **Palette decision** (in ADR-025):
  - 16 slots, 4 biome groups × 4 textures, from Gemini's research §
    "Bundled textures" verbatim. Reproduce the table in ADR-025 with:
    slot id (00–15), name, biome group, ambientCG asset id, source URL,
    licence (must be CC0-1.0), default `tex_scale` (start at 0.02 per
    Claude+Gemini agreement), default `tex_mult` (1.0).
  - Each slot includes BOTH diffuse and normal map. Diffuse can be JPG
    (smaller); normals MUST be PNG (JPG 4:2:0 destroys X/Y vectors).
  - Reject anything non-CC0. Reject Poly Haven unless it's their CC0 BY
    licence. Reject OpenGameArt (mixed licences).
- **`scripts/fetch-textures.sh`**:
  - sha256-pinned per ZIP. Failed checksum → script exits 1.
  - Downloads each ZIP from ambientCG into a temp dir.
  - Extracts ONLY `*_Color.*` (diffuse, JPG preferred for size) and
    `*_NormalGL.*` (normal, PNG required). Discards roughness / AO /
    displacement / metallic.
  - Writes to `tools/textures/<NN-slot-name>/{diffuse.{jpg,png},
    normal.png, meta.toml}`.
  - `meta.toml` schema:
    ```toml
    name = "Grass meadow"
    biome = "Earth-Temperate"
    source = "https://ambientcg.com/view?id=Grass012"
    license = "CC0-1.0"
    default_tex_scale = 0.02
    default_tex_mult = 1.0
    ```
  - Idempotent: re-running with all slots present and checksums matching
    is a no-op (~3 s of stat calls).
  - CI-time URL HEAD-check helper (separate flag, e.g.
    `./scripts/fetch-textures.sh --check`): verifies each URL still
    returns 200 + the ZIP still contains the expected members (ambientCG
    re-numbers assets occasionally — Claude open Q #2).
- **`CREDITS.md`** at repo root:
  - One section per asset source (ambientCG, Poly Haven if used,
    Beherith for tooling courtesy).
  - Per-asset list pointing to ambientCG URLs.
  - Note that all bundled textures are CC0-1.0; no per-texture
    attribution UX required in-app.
- **`docs/DECISIONS.md`**:
  - ADR-025: starter texture pack (palette table + sourcing decisions +
    licence policy + rationale for adopting Gemini's bundle-the-normal
    over Claude's synthesise-from-luminance).
  - ADR-027 (registry layout): if ADR-025 stays focused on "what's in
    the pack," ADR-027 captures the on-disk layout
    (`tools/textures/<slot>/meta.toml` schema + how D3's registry will
    scan it at runtime). Fold into ADR-025 if combined stays under
    ~300 lines — judgement call.

**Out of scope** (later sprints):
- The barme-core::splat module (D3 / Sprint 8).
- The DNTS bake pipeline (D2 / Sprint 8).
- The splat fragment shader (D4 / Sprint 9).
- The splat tool UI (D5 / Sprint 9).
- The splat pipeline wiring (D6 / Sprint 11).
- Any runtime code change beyond the fetch script.

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells (though this sprint has no
  cargo work).
- Before every commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects.
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing: N/A (no runtime code).
- Devlog folder per item.

## Step 5 — Out of scope

- All D2+ items. This sprint ships fetch tooling + decision, nothing else.
- Editing CLAUDE.md to reference texture-pack assumptions — defer until D3
  lands the registry crate.
- Auto-running the fetch script in CI — design only; CI hookup is later.

## Step 6 — Critical pitfalls (read twice)

From phase-3-plan.md D1 + the research digests:

1. **License hygiene**: bundling anything non-CC0 (GPL / share-alike)
   would poison the user's `.sd7` output. CC0-1.0 only, period. Poly
   Haven assets vary — verify each individual asset's licence on the
   product page, don't assume "Poly Haven = CC0."

2. **JPEG normal maps are silently wrong**. 4:2:0 chroma subsampling
   destroys X/Y vectors. Force PNG for `*_NormalGL.*` extraction. Diffuse
   may be JPG.

3. **ambientCG asset renumbering**: asset IDs aren't stable across re-
   processing (e.g. Ground037's color map was re-processed Jan 24 2021).
   Fetch script needs HEAD-check + member-presence verification; design
   the `--check` flag accordingly. Without this, a year-later run will
   silently 404 on some slots.

4. **`tools/textures/` is gitignored** per CLAUDE.md "Things deliberately
   NOT in this repo." Add it to `.gitignore` if not already there. Verify
   with `git status` after running the fetch script — only the script,
   ADRs, and CREDITS.md should show up.

5. **`default_tex_scale = 0.02`** is the engine-historical default
   (`SMF_DETAILTEX_RES = 0.02` in
   `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl:25`).
   Real BAR maps use 0.0015–0.008 per channel (Enceladus: `[0.004,
   0.007, 0.008, 0.0015]`). 0.02 is correct as a baseline default but
   the D5 UI in the Splat inspector should surface the smaller
   real-world range — Sprint 9 wires that as a tooltip on the
   `ramp_slider_labelled` for `tex_scale`. Note in ADR-025.

10. **Normal-map convention is OpenGL tangent space** —
    `SMFFragProg.glsl:276-278` builds the TBN from the per-fragment
    normal, then decodes each splat texture as `* 2 - 1`. OpenGL
    convention's Y points up; the Y-flip (D2's job) inverts the green
    channel of DirectX-source normals. ambientCG ships
    `*_NormalGL.*` (already OpenGL); ensure the fetch script extracts
    the GL variant, not the DX variant.

6. **Single CREDITS.md is enough** under CC0 — no per-texture in-app
   attribution UI needed. Don't over-engineer.

7. **`splatDetailNormalDiffuseAlpha = 0` baseline** (Gemini's safer
   default). The high-pass-diffuse-in-alpha workflow is deferred to
   ADR-034 once the splat preview lands in D4. Don't bake the high-pass
   into the fetch script.

8. **Asset filename hygiene**: ambientCG ZIPs vary in member naming
   conventions (`Ground012_1K_Color.jpg`, `Ground012_2K_Color.png`,
   etc.). The fetch script's extractor should glob `*_Color.*` /
   `*_NormalGL.*` rather than hardcoding filenames. Pick the 1K
   resolution variant where multiple resolutions exist (1024² is the
   BAR-normative tile size per both research reports).

9. **Renaming asset slots later is painful** — the meta.toml is the
   contract D3's registry depends on. Get the slot id + name right
   first time. Slot ids are 00..15 in source order; names are short-
   kebab-case (e.g. `grass-meadow`, `metal-brushed`).

## Step 7 — Exit criteria

- 2 commits on `main`: D1 (script + ADR + CREDITS) + rollup.
- 1 devlog folder filled.
- D1 checkbox ticked in phase-3-plan.md.
- ADR-025 (and possibly ADR-027) in `docs/DECISIONS.md`.
- `scripts/fetch-textures.sh` exists and is executable.
- `CREDITS.md` exists at repo root.
- Running `./scripts/fetch-textures.sh` populates `tools/textures/` with
  16 slot directories, each with `{diffuse.*, normal.png, meta.toml}`.
- Running `./scripts/fetch-textures.sh` a second time is a no-op.
- Running `./scripts/fetch-textures.sh --check` exits 0 (all 16 URLs
  still 200, ZIPs contain expected members).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green (no code changes, but verify the workspace still builds).
- `git status` after fetch shows ONLY the new script + ADR + CREDITS.md
  changes; `tools/textures/` is gitignored and absent from the diff.
- Final devlog log summarising what shipped + "Sprint 8 = D2 + D3 (DNTS
  bake pipeline + splat module)" handoff note.

Start by reading the two research-findings files. The palette decision
flows from there. Then draft ADR-025 in a scratchpad before writing the
fetch script — the script is mechanical once the palette is locked.
