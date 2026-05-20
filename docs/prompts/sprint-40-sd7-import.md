# Sprint 40 — F13 .sd7 import / decompile

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 40** — implements **F13** (.sd7 import / decompile)
from the SRS Stage-2 list. Today the editor is one-way: project →
.sd7. F13 reverses the flow: load any `.sd7` file from the BAR
multiplayer pool and reconstruct an editable `Project`.

**Critical caveat**: full decompilation has fidelity limits. We
recover:
- Heightmap (from SMF chunks).
- Metalmap (from SMF; but since Sprint 5 / C2 emits all-zero
  metalmaps, real-world maps have non-zero metalmaps that need
  conversion).
- `mapinfo.lua` (parsed back via Lua AST).
- Feature placements (from `featureplacer/features.lua`).
- Splat distribution (1024² PNG if present).

We do NOT recover:
- The exact authoring intent (e.g., which layers composed the
  final diffuse — that data isn't in the .sd7).
- Original DNTS slot assignments (only the baked .dds files).

After this sprint, the user can open a `.sd7`, edit it, and
re-publish.

**Prerequisites:**
- Stage 1 + renderer-parity arc complete.
- Sprint 21 lint pass — re-imported projects often have lint
  warnings; the pass catches them.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F13.
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — §12 (.sd7
   fidelity caveats).
4. Reference: `RecoilEngine/rts/Map/SMF/SmfMapFile.cpp` — SMF
   binary format.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-40-sd7-import
```

## Step 3 — Scope

### 1. .sd7 (7z) extraction

`crates/barme-pipeline/src/import.rs` (new):

```rust
pub fn import_sd7(sd7_path: &Path, project_dir: &Path) -> Result<Project, ImportError>;
```

Step 1: extract the .sd7 (it's a 7z archive). Use `sevenz-rust2` or
shell out to the bundled `tools/7z`. Yields a staging dir.

### 2. SMF parser

`crates/barme-pipeline/src/smf_reader.rs` (new): parse the SMF
binary format:
- Header (magic + version + dims).
- Heightmap chunk (16-bit).
- Type-map chunk (8-bit).
- Mini-map chunk (1024² DXT1).
- Metal-map chunk (8-bit).
- Feature placeholder chunk (often empty since features come from
  Lua).

### 3. Mapinfo.lua reverse-parser

`crates/barme-pipeline/src/mapinfo_parse.rs` (new): parse a Lua
`mapinfo.lua` file back to the typed `MapInfo` schema.

Use the same Lua AST module from Sprint 5 / C2 (`barme-pipeline/src/lua_ast.rs`)
but in reverse: AST → typed struct. Handle the audit-corrected
keys (sundir AND sunDir; skyAxisAngle; etc.) and migrate to
canonical forms.

### 4. Feature import

Parse `mapconfig/featureplacer/features.lua` and
`mapconfig/map_metal_layout.lua`. Reconstruct `Project.features`
and `Project.metal_spots`.

### 5. Layer stack synthesis

The diffuse from the .sd7 (extracted from SMT chunks) becomes a
**single base layer** in the new `LayerStack`. Subsequent paint
work happens on top.

The baked DNTS .dds files become bound layers (via Sprint 17 /
ADR-041 hybrid emission). Each .dds gets a "Slot 00 (imported
from MapName)" layer.

### 6. UI — File menu Import

`File > Import .sd7…` → file picker. After import:
1. New project window opens with the reconstructed state.
2. Toast: "Imported MapName.sd7 with N warnings — open lint panel".
3. Lint typically surfaces several warnings (metal-spot vs
   metalmap conflict, etc.) — the user reviews and decides.

### 7. Tests + rollup

- **Round-trip via build → import**: build a default project →
  import the resulting .sd7 → re-build → assert behavioural
  equivalence (heightmap pixel-identical; features identical;
  mapinfo round-trip via emit → parse → emit cycle byte-identical).
- **Real-world fixture**: import Comet Catcher Remake's .sd7;
  open in editor; verify it doesn't panic + lint warnings are
  expected/documented.
- **Rollup**: STATUS UPDATEs (F13 done; Stage 2 progressing).

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on import stages with
timings; `warn!` on lossy reconstructions; `error!` on parse
failures (with full stderr).

## Step 5 — Out of scope

- **Importing layered painter state from non-editor-source .sd7s**
  — the .sd7 has only the baked output; no way to recover the
  layer stack. We synthesise a single-layer stack.
- **DXT1 decode** — use `bcdec_rs` from Sprint 29.
- **Validating against the BAR mod's `_def` types** — defer to
  a future lint sprint.

## Step 6 — Critical pitfalls

1. **`64·N+1` invariant**: imported heightmap dims must match.
   If not, error out with a clear "this .sd7 has dims X×Y which
   don't match the BAR convention — corrupted or non-BAR file."

2. **Metal-spot vs metalmap conflict** (PITFALLS §13): the .sd7
   has both. Sprint 21 lint warns. Resolution: prefer Lua spots;
   import any non-zero metalmap pixels as additional spots with
   conservative metal values.

3. **Mapinfo round-trip fidelity**: not every Lua file round-trips
   to byte-identical output. The emit-parse-emit cycle should
   converge after 2 emits. Test.

4. **Lua AST coverage**: hand-curated .lua files may have
   constructs the AST doesn't handle (e.g., `function()` calls
   for procedural mapinfo). Surface unknown constructs as
   warnings, copy verbatim into `mapinfo_overrides`.

5. **Encoding**: .lua files may be UTF-8 BOM'd. Strip the BOM.

## Step 7 — Exit criteria

- 6+ commits on `main`: .sd7 extract, SMF reader, mapinfo
  reverse-parser, feature import, layer synthesis, UI + rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (F13 done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: import Comet Catcher Remake → open in editor →
  render correctly → re-build → identical output.
- Final devlog: summary + "Sprint 41 = F14 v2 (FBM + river carve)"
  handoff.
