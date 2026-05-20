# Sprint 10 — Mapinfo audit corrections (PITFALL §11/12/18/19/20)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 10** — a small bugfix-only sprint that closes the
mapinfo-emitter gap surfaced by the 2026-05-18 source audit
(`docs/research/source-audit-2026-05-18/FINDINGS.md`). The fixes are
load-bearing: the emitter currently writes a `mapinfo.lua` that
publishes a BAR map BAR's `unit_sunfacing.lua` gadget cannot consume
(silent nil-deref → "waiting for players" hang), and uses two
deprecated/unused engine keys. PITFALLS.md §11–21 captures the full
audit; this sprint addresses the five emitter-side items.

**Prerequisites:** Sprints 1–6 done. Sprint 7–9 may or may not be
done — this sprint touches `crates/barme-core/src/mapinfo_schema.rs`
and `crates/barme-pipeline/src/mapinfo.rs`, NOT the splat / texture
pipeline. Run it before Sprint 12 (D6 / splat emission) ships
otherwise D6's first-build `.sd7`s will inherit the bugs into shipped
maps.

**Why one sprint and not "fold into Sprint 9 / 12":** keeping mapinfo
bugfixes isolated keeps the diff small + reviewable, and lets the
unit-sunfacing nil-deref fix land independently of any splat work. The
emitter is the most-tested module in the project; touching it as a
discrete commit is cheap.

## Step 1 — Read the context

Read these in order:

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.3 (mapinfo schema /
   three-gate model). The 2026-05-18 STATUS UPDATE block (search
   "BAR source audit") flags all five items addressed here.
3. **`/home/teague/code/BARMapEditor/docs/PITFALLS.md` §11–21** —
   the source-audit additions. **All five items in this sprint trace
   directly to PITFALL numbers; cite them in commit messages.**
4. `/home/teague/code/BARMapEditor/docs/research/source-audit-2026-05-18/FINDINGS.md`
   — §1.1 (`voidAlphaMin`), §1.3 (`skyDir` deprecated), §1.4
   (`sundir`/`sunDir` case), §1.11 (`minimapRotation` unused),
   §12 NEW-1 through NEW-10 summary.
5. `/home/teague/code/BARMapEditor/crates/barme-core/src/mapinfo_schema.rs`
   — current schema state. Look for: `pub sky_dir`,
   `pub sun_dir: SunDir`, `pub minimap_rotation`, and the absence of
   `void_alpha_min`.
6. `/home/teague/code/BARMapEditor/crates/barme-pipeline/src/mapinfo.rs`
   — current emitter. Touch points: `atmosphere_block` (`skyDir`
   emission, line ~209), `lighting_block` (single `sunDir` emission,
   line ~186), `gui` block (`minimapRotation`, line ~388), and the
   `lighting_sundir_key_is_camel_case_not_lowercase` regression test
   (line ~751) — which currently asserts the WRONG thing.
7. `/home/teague/code/BARMapEditor/scratch/bar-maps/extracted/comet/mapinfo.lua`
   — a real BAR map. Note line 328: `lowerkeys(mapinfo)`. Real BAR
   maps lowercase every key after definition; the engine reads
   case-insensitively via `LuaParser` (`rts/Lua/LuaParser.cpp:283`,
   `lowerKeys=true` default + `LuaUtils::LowerKeys` pass on load).
   This is why writing only camelCase + omitting the `lowerkeys()`
   call breaks gadgets — they access `mapinfo.lighting.sundir`
   directly via Lua, bypassing the case-insensitive C++ path.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-mapinfo-audit-fix
./devlog/log.sh log stage-1-mapinfo-audit-fix "starting"
```

Fill `stage-1-mapinfo-audit-fix/`:
- `goals.md` — bullet list of the 5 fixes (sundir+sunDir, skyAxisAngle,
  drop minimapRotation, sunDir.w=1.0, voidAlphaMin schema).
- `notes.md` — design choice for the sundir/sunDir dual emit (write
  both, no `lowerkeys()` call — see Step 6 pitfall #1).
- `logs/<timestamp>__<title>.md` — session logs.

## Step 3 — Scope

One commit per fix, then a rollup. Five fixes + rollup = 6 commits.
Order matters: schema first (no behavioural change), then emitter, then
the regression-test inversion.

### Fix 1 — schema additions [PITFALL §20, FINDINGS §1.1]

`crates/barme-core/src/mapinfo_schema.rs`:

- Add `pub void_alpha_min: f32` to the top-level `MapInfo` struct
  with `#[serde(default = "default_void_alpha_min")]`.
- `fn default_void_alpha_min() -> f32 { 0.9 }` matches engine
  default (`MapInfo.cpp:107`).
- `MapInfo::bar_default()` sets `void_alpha_min: 0.9`.
- Unit test: `bar_default().void_alpha_min == 0.9`.

### Fix 2 — emit `voidAlphaMin` [PITFALL §20]

`crates/barme-pipeline/src/mapinfo.rs`:

- In the top-level emit block (where `voidWater` / `voidGround`
  emit), add `push_f32(&mut t, "voidAlphaMin", info.void_alpha_min);`.
- Emit only when `info.void_ground == true` per PITFALL §20's "F9
  surfaces it only when voidGround = true" — keep the no-op case
  noise-free.
- Unit test: `bar_default()` with `void_ground = true` emits
  `voidAlphaMin = 0.9`; default (`void_ground = false`) does not.

### Fix 3 — `sundir` + `sunDir` dual emit [PITFALL §11, FINDINGS §1.4]

`crates/barme-pipeline/src/mapinfo.rs::lighting_block` (~line 186):

```rust
let sundir = sundir_value(b.sun_dir);
let mut t = vec![
    (LuaKey::str("sunDir"), sundir.clone()),
    (LuaKey::str("sundir"), sundir),  // BAR-mod-gadget compat
];
```

**Why both, not just lowercase:** the engine's `LuaParser` lowercases
keys on load (`rts/Lua/LuaParser.cpp:283`), so `sunDir` alone WOULD
work for the engine. But BAR gadgets (e.g.
`luarules/gadgets/unit_sunfacing.lua:43`) read `mapinfo.lighting.sundir`
directly via Lua VFS.Include, NOT through the engine's case-folding
path. Writing both keeps both consumers happy without depending on a
maphelper `lowerkeys()` call in the emitted file. Cite PITFALL §11
in the commit message.

**Invert the existing regression test** at line ~751:

```rust
#[test]
fn lighting_emits_both_sundir_keys() {
    let info = MapInfo::bar_default();
    let s = render_mapinfo(&info);
    assert!(s.contains("sunDir = {"),
        "expected camelCase sunDir; got:\n{s}");
    assert!(s.contains("sundir = {"),
        "expected lowercase sundir alongside camelCase; got:\n{s}");
    // Both must point at the same 4-float value (smoke check via
    // string-equal subslices of each).
}
```

Delete or repurpose `lighting_sundir_key_is_camel_case_not_lowercase`
— its premise was inverted by the source audit.

### Fix 4 — `sunDir.w = 1.0` default [PITFALL §18, FINDINGS §1.4]

`crates/barme-core/src/mapinfo_schema.rs` line ~496:

```rust
// before:
sun_dir: [0.3, 1.0, -0.2, 1.0e9],
// after:
sun_dir: [0.3, 1.0, -0.2, 1.0],
```

Engine default is `float4(0.0, 1.0, 2.0, 1.0)` — `w=1.0` is an
intensity scalar; `1.0e9` was a stale sunStartDistance leakage from a
different code path and over-saturates sunlight on load (FINDINGS
NEW-6). Keep the xyz direction the editor's existing default; only
the W changes.

Unit test: `bar_default().lighting.sun_dir[3] == 1.0`.

### Fix 5 — `skyDir` → `skyAxisAngle` [PITFALL §12, FINDINGS §1.3]

`crates/barme-core/src/mapinfo_schema.rs::AtmosphereBlock`:

- Rename `pub sky_dir: [f32; 3]` to `pub sky_axis_angle: [f32; 4]`.
- Default to `[0.0, 0.0, 1.0, 0.0]` (engine default per
  `MapInfo.cpp:149`: axis = +Z, angle = 0 radians).
- Custom `Deserialize` migration: legacy `[[atmosphere.sky_dir]]`
  files materialise into `sky_axis_angle = [sky_dir[0], sky_dir[1],
  sky_dir[2], 0.0]`. Preserves the direction; sets angle to 0.

`crates/barme-pipeline/src/mapinfo.rs::atmosphere_block` (~line 209):

- Replace `push_rgb(&mut t, "skyDir", b.sky_dir);` with
  `push_f32_array(&mut t, "skyAxisAngle", &b.sky_axis_angle);`.

Unit test:
- Rendered `mapinfo.lua` contains `skyAxisAngle = { 0, 0, 1, 0 }`.
- Rendered `mapinfo.lua` does NOT contain `skyDir = ` (engine logs
  `L_DEPRECATED` if present).

### Fix 6 — drop `minimapRotation` [PITFALL §19, FINDINGS §1.11]

`crates/barme-pipeline/src/mapinfo.rs` line ~388:

- Delete the `(LuaKey::str("minimapRotation"), LuaValue::Int(...))`
  emit. The engine reader at `MapInfo.cpp:119-124` only consumes
  `autoShowMetal`; `minimapRotation` is dead Lua.
- Schema: if `MapInfo` carries a `minimap_rotation` field, mark it
  `#[deprecated]` with a comment pointing at PITFALL §19, AND emit
  nothing — or remove the field. Prefer remove unless the F9 form
  (C7 / Sprint 13) needs a legacy-compat surface.

Unit test: rendered output does NOT contain `minimapRotation`.

### Fix 7 (rollup) — STATUS UPDATEs + ROADMAP

- `SRS.md`: add a STATUS UPDATE 2026-MM-DD under §1.3 noting Sprint 10
  closed the audit emitter gap. Cite PITFALL §11/12/18/19/20.
- `docs/ROADMAP.md`: no F-number reflects this directly — append a
  one-line bullet under the Phase 3 / Sprint 6 entry: "Source audit
  emitter corrections applied (Sprint 10 → devlog
  stage-1-mapinfo-audit-fix)."
- `devlog/stage-1-mvp/phase-3-plan.md`: add a new "Stream X — Audit
  corrections" section or append the fix list to the C3 entry's
  STATUS UPDATE area (decision per house rule #1).
- Final devlog log summarising what shipped + "Sprint 11 = C4 + C5
  (metal-spot + geo-vent placement tools)" handoff note.

## Step 4 — Standing constraints

- `source ~/.cargo/env` in fresh shells.
- Before every commit: `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All green.
- No `Co-Authored-By: Claude` trailer.
- Terse commit subjects (e.g. `mapinfo: emit sundir alongside sunDir (PITFALL §11)`).
- Local-only.
- SRS is source of truth — STATUS UPDATE on contradiction.
- Tracing: N/A (no runtime / pipeline-orchestration code).
- Devlog folder per item (one folder for the whole sprint is fine —
  the 5 fixes are tightly coupled).

## Step 5 — Out of scope

- The C8 lint pass that surfaces these to the user. That's Sprint 14.
- F9 form editor wiring for `voidAlphaMin` / `skyAxisAngle`. That's
  Sprint 13 (C7).
- Any splat-side changes — splat ADRs (025/026/035/036) own their
  own emitter for `splatDetailNormalTex`. Sprint 12 wires that.
- `splatDetailNormalTex` subtable form (PITFALL §15). That belongs in
  Sprint 12 / D6 emission wiring.
- Modtype as a typed enum (PITFALL §21). Pure cosmetic; do it when
  the F9 form editor wires the field (Sprint 13).

## Step 6 — Critical pitfalls (read once)

1. **Don't add a `lowerkeys(mapinfo)` call to the emitter's output**
   as an alternative to dual-emit. Two reasons: (a) emitting both
   keys is cheaper to verify in unit tests; (b) `lowerkeys()` is a
   function provided by `maphelper.sdz` — depending on it implies a
   silent contract with the BAR mod's depend chain. We already write
   `depend = { "Map Helper v1" }` (per C1 default), so the function
   IS available, but the dual-emit path keeps the option open to
   drop the dependency later without a regression.

2. **Don't migrate `sky_dir` data lossily.** A pre-Sprint-10
   `.barmeproj` with `[[atmosphere.sky_dir]]` and no `sky_axis_angle`
   field must load forward via `serde(default)` + the custom migration
   (above). The wizard's default `skyDir` value (which the legacy
   emitter wrote) becomes the new schema's `skyAxisAngle.xyz` with
   `w = 0`. Test the migration with a pinned legacy fixture.

3. **Don't reuse ADR numbers.** ADR-025/026/027/028/029/030/031/032/
   033/035 are all taken. ADR-034 is reserved for splat
   diffuse-in-alpha (deferred). The audit fixes don't need an ADR —
   they're regression fixes against an existing schema, not new
   decisions. Reference PITFALLS.md §11–21 in commit messages.

4. **Don't break the determinism test.** Each emitter sub-block is
   byte-identical across repeated renders. Adding `voidAlphaMin` /
   `skyAxisAngle` keys, removing `minimapRotation`, and dual-emitting
   `sundir` / `sunDir` all need to preserve canonical key ordering
   (alphabetical within table). Run
   `determinism_repeated_render_byte_identical` after each fix.

5. **PITFALL §11's claim that "the engine reads ONLY camelCase" is
   technically inaccurate** (the LuaParser lowercases on load), but
   the DEFENSIVE PRACTICE — emit both — is still right. Don't
   rewrite the pitfall text; the action item stands. If the pitfall
   wording bothers a reviewer, point them at FINDINGS §1.4 + this
   sprint's notes.md for the deeper trace.

## Step 7 — Exit criteria

- 6 commits on `main`: schema additions, voidAlphaMin emit,
  sundir+sunDir dual emit (+ test inversion), sunDir.w fix,
  skyDir→skyAxisAngle rename, drop minimapRotation. Rollup commit
  optional — STATUS UPDATEs can fold into the last commit.
- 1 devlog folder filled.
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green at each commit.
- Render-output snapshot: `MapInfo::bar_default()` →
  `render_mapinfo(&info)` produces a Lua string that:
  - Contains BOTH `sunDir = {` AND `sundir = {`.
  - Contains `skyAxisAngle = {`.
  - Does NOT contain `skyDir = `.
  - Does NOT contain `minimapRotation`.
  - Contains `voidAlphaMin = 0.9` (when `void_ground = true`).
  - `lighting.sun_dir[3] == 1.0` in the schema instance.
- One smoke build (optional but recommended): `cargo run -p barme-app`,
  create a wizard project, Build & Install, load the `.sd7` in BAR.
  Confirm no "waiting for players" hang and no
  `attempt to index field 'lighting' (a nil value)` in Recoil's log.
- Final devlog log summarising what shipped + "Sprint 11 = C4 + C5
  (F5 metal-spot placement + F6 geo-vent placement)" handoff note.

Start by reading `crates/barme-pipeline/src/mapinfo.rs` end-to-end —
the emitter is small (~800 lines including tests) and a single pass
will surface every edit site. Do the schema-only fix first
(`voidAlphaMin` + `sunDir.w`) since it has the smallest blast radius;
the renames + dual-emit need the schema in place anyway.
