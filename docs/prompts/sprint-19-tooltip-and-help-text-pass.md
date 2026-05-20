# Sprint 19 — UI tooltip + help-text pass + validation-chip click + status wiring (U1)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 19** — the first of three focused UI/UX polish sprints
(19 / 20 / 22). It is a **discoverability and feedback** sprint. The
2026-05-20 UX audit found:

- Only **36 `.on_hover_text()` calls in the entire app**, for ~50
  interactive widget kinds. Coverage ~30 %.
- The status strip displays a **hard-coded `"0 issues"`** label
  (`crates/barme-app/src/main.rs:6028`) — the real `validation_summary`
  exists but is never wired.
- The top-bar validation chip has **no click affordance and no
  hover-text**, despite having 8 possible states ranging from "No
  heightmap" to "DNTS + water: LOS bug." Users have no path to learn
  what these mean.
- The `viewport_chrome` toolbar tooltips claim non-existent keyboard
  shortcuts (`G`, `L`, `W` — all of which conflict with tool
  accelerators). False promises.
- Paint-mask brushes fall back to `LIGHT_GRAY` in `overlay.rs:328`
  because `brush_ring_color` only handles the three sculpt brushes —
  the user has no semantic colour feedback when painting layers.

After this sprint, every interactive widget in the app has an
`.on_hover_text` with what it does, units / range, BAR consequence,
and keyboard shortcut where applicable. The validation chip is
clickable and opens a stub lint panel (which Sprint 21 populates).
The status strip shows live issue counts.

This sprint is **mechanical and high-impact**. There is no new ADR.
Every change is additive and localised.

**Prerequisites:**
- Sprint 18 complete. The F9 mapinfo form lands a lot of new
  widgets; this sprint catches them in the tooltip net.
- All previous sprints (1–17) shipped.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §3.3 (NFR-Discoverability,
   §1.4 user stories — the wizard / sculpt / paint flows must each be
   discoverable without docs), §2.1 (the pitfall list; tooltips
   surface BAR consequence of each field).
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — every numbered
   pitfall is a candidate "BAR consequence" line in a tooltip. Cite
   the section number in the tooltip text where appropriate (e.g.
   `extractor_radius = 500` tooltip → "Engine default; BAR
   overrides to 80. PITFALL §6.").
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs` —
   the bulk of the work lives in the `inspector_*` functions
   (search for `fn inspector_`). Walk every DragValue, ComboBox,
   Button, Slider, color_edit_button, and Chip.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/layers_panel.rs`
   — the most complex egui surface. 8 tooltips today; aim for >25.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/paint_view.rs`
   — 0 tooltips today. The mask preview chip, status strip,
   middle-drag pan, and scroll-wheel zoom all need help-text.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/viewport_chrome.rs`
   — false-promise tooltips at lines 268-289. Fix or remove.
8. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/overlay.rs`
   — `brush_ring_color` (line 323) handles only 3 sculpt brushes;
   extend to cover mask-reveal / mask-hide / mask-smooth / mask-fill.
9. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/widgets.rs`
   — extend `section / chip / ramp_slider_labelled / pill_toggle`
   to support an optional `hover_text` parameter.
10. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/cheat_sheet.rs`
    — read the existing keyboard map; tooltips must cite the same
    shortcut strings.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-ui-tooltip-pass
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. Help-text catalogue + hover-text widget extensions

**New module:** `crates/barme-app/src/ui/help_text.rs`. A flat
`pub fn help(id: HelpId) -> &'static str` keyed by a `HelpId` enum.
Centralises the strings so Sprint 22's tour mode can re-use them.

```rust
pub enum HelpId {
    // Project header
    ProjectName,
    MapSizeSmuX,
    MapSizeSmuZ,
    HeightScale,
    SavedChip,
    HeightmapChip,
    // Sculpt
    SculptBrushOff, SculptBrushRaise, SculptBrushLower, SculptBrushSmooth,
    SculptRadius, SculptStrength, SculptFalloff,
    // Metal
    ExtractorRadius, MetalSpotMetal, MetalSpotX, MetalSpotZ,
    // ... and so on for every widget kind
}
```

Convention: each string ends with "[Shortcut: Ctrl+S]" when a
keyboard chord exists, and "[PITFALL §N]" when a pitfall directly
applies. Strings are 1-3 sentences.

Extend `widgets.rs`:
- `section(ui, title, accent, hover_text: Option<&str>)` — section
  headers gain optional hover-text.
- `chip(ui, label, tone, hover_text: Option<&str>)`.
- `ramp_slider_labelled` — already takes a `label`; add optional
  `hover_text` param.

**Acceptance:** the help_text module compiles, has at least 80
entries, and is referenced from every Inspector function below.

### 2. Inspector tooltip pass — one commit per tool

One commit per tool (or group them by stream — judgement call).
For each:

- **`inspector_select`** (main.rs:6322): add hover-text to all
  rendered widgets including the "Orbit camera" instructions.
  Mention scroll = zoom, MMB drag = pan, RMB drag = orbit.
- **`inspector_sculpt`** (main.rs:6974): 4 brush cards each get
  tooltips. Radius / Strength / Falloff sliders get unit + range +
  effect ("strength = 1.0 means full brush height per stamp at
  centre"). The Symmetry chip at the top is already there but no
  tooltip — add one explaining the current mode.
- **`inspector_metal`** (main.rs:6344): row delete buttons get
  "Delete spot — Ctrl+Z to restore"; coordinates X/Z get "elmos
  from south-west corner"; metal value gets BAR convention
  (0.5 perimeter / 2.0 standard / 4.0-5.2 central).
- **`inspector_geo`** (main.rs:6528): a tooltip on the section
  header explaining "Geo vents = unique economy slots that produce
  steam plumes; BAR's `geovent` FeatureDef registers via
  `Spring.GetAllFeatures()`." Per-row coords get the same treatment.
- **`inspector_feature`** (main.rs:6646): category ComboBox gets a
  tooltip explaining each category (trees / rocks / props / wreckage).
  Filter TextEdit gets a placeholder ("filter by name, display,
  or tag"). Rotation DragValue gets "BAR heading: 0° = facing
  south; 16-bit fixed point — `Spring.CreateFeature(..., heading)`
  expects this raw integer".
- **`inspector_paint_layer`** (main.rs:6904): four brush cards
  each get **distinct** tooltips ("Reveal: increases the active
  layer's mask alpha"; "Hide: decreases"; "Smooth: blurs mask
  values"; "Fill: sets mask to target value (LMB drag) — bypasses
  symmetry"). Radius / Strength / Spacing get units (elmos). The
  `fill_target_visible` checkbox and `mask_only_preview` checkbox
  get tooltips citing the implication.
- **`inspector_water`** (main.rs:7109): preset chips get per-preset
  hover-text describing what each preset changes (Pacific = blue
  ocean / Tropical = teal / Acid = green / Lava = orange-red /
  Magma = lava with stronger fog). Damage DragValue clarifies
  "HP per game tick (30 ticks/sec) when a unit touches water".
  Floor / Ceiling get the same tooltips as the header counterpart.
- **`inspector_start_positions`** (main.rs:7731): preset ComboBox
  gets per-preset descriptions (`OneVOne` / `EightVEight` /
  `ThreeWayFFA` / `FourWayFFA`). Drag-paint count clarifies "LMB
  drag a line; N positions distributed equally along it". Color
  swatch hover: "AllyGroup colour drives the canvas marker AND
  the minimap dot AND the in-game player colour".
- **`inspector_procgen`** (main.rs:7972): preset chips each get a
  description (Parabolic bowl / Saddle / Diagonal ramp / Plateau /
  Custom). Domain Unit / Centered toggle explains the difference
  (Unit: `(x,z) ∈ [0,1]`; Centered: `(x,z) ∈ [-1,1]`). Commit
  button gets "Replaces the current heightmap; Ctrl+Z reverts".
- **`inspector_header`** (main.rs:6203): project name TextEdit
  gets sanitisation rules; map size `smu_x` / `smu_z` get
  `64·N+1` pixel math; height_scale gets "BAR maps cap at
  ~4096 elmos"; Saved/Unsaved chip gets last-save timestamp
  via hover; Heightmap Valid/Invalid chip explains the cause
  when Invalid.

For each tool, add a sticky **Symmetry + Map size** chip row at
the top of the inspector body (below the header strip but above
the first section), echoing the global symmetry mode for that
tool's strokes. Hover gives the full symmetry description.

Wire `inspector_mapinfo` (from Sprint 18 / C7) too — if its
tooltips are already in place, harvest them into `help_text.rs`
and dedupe.

### 3. Validation chip click + status-strip wiring

**Validation chip** (`main.rs:5999`):
- Add `.on_hover_text(...)` listing the issue summary
  ("`validation_summary` → 0 issues" / "2 issues: DNTS+water LOS;
  no specular").
- Wire `.clicked()` to open a `LintPanel` Window (new module
  `crates/barme-app/src/ui/lint_panel.rs`). For Sprint 19 the
  panel is **a stub** — it renders the current
  `App::validation_summary` output as a list. Sprint 21
  populates it with `LintRule` variants.
- The chip's coloured dot reflects severity (red = error,
  amber = warning, neutral = info). The 8 existing
  `validation_summary` states map onto these.

**Status strip "0 issues"** (`main.rs:6028`):
- Replace the hard-coded `"0 issues"` with a live count derived
  from `validation_summary().len()`.
- Clickable → opens the same `LintPanel`.

**Top-bar Help (?) icon** (new):
- Place to the right of the validation chip. `icon_button(Icon::Help)`.
- Click opens the cheat-sheet modal (currently only reachable via
  `?` chord). Sprint 22 extends this into a full help center.

**Recenter / save / build button tooltips** — current Save (line
5985) has no tooltip and the `•` dirty dot is undocumented; add a
tooltip "Save project — Ctrl+S. Unsaved changes shown by the dot."
Build button (already has a split-button widget) needs a tooltip
listing the destination `.sd7` path.

### 4. False-promise tooltip cleanup + brush-ring colour fix

**`viewport_chrome.rs:268-289`:**
- The `G` / `L` / `W` keyboard shortcuts referenced in tooltips
  conflict with tool accelerators. Either (a) remove the shortcut
  from the tooltip text, or (b) wire the global accelerator if it
  makes sense (Lighting toggle on Shift+L for example).
- Audit each tooltip; remove any that names a shortcut not bound
  in `handle_keyboard`.

**`ui/overlay.rs:323` (`brush_ring_color`):**
- Extend to handle mask-reveal, mask-hide, mask-smooth, mask-fill
  (Sprint 16 brush kinds). Map: reveal = `t.success` green,
  hide = `t.danger` red, smooth = `t.accent` blue, fill = `t.warn`
  amber. The colour MUST match the four cards in
  `inspector_paint_layer` (main.rs:6917-6922) — visual rhyme.

**`ui/minimap.rs:131-136` (symmetry guide):**
- The minimap currently draws an unconditional vertical bisector.
  Wire it to the real `Project.symmetry` (Horizontal / Vertical /
  Quad / Diagonal-XX / Rotational-N). Multiple-axis modes draw all
  axes / spokes. The mockup commentary in the source can stay as a
  history note but the behaviour matches reality.

### 5. Tests + smoke run + rollup

- **Hover-text count test**
  (`crates/barme-app/src/ui/help_text.rs::tests`): asserts the
  `HelpId` enum has ≥80 variants and every variant maps to a
  non-empty string.
- **Round-trip test** that the `LintPanel` Window opens on chip
  click (egui smoke test pattern — render a fake state, assert
  the window opens after the click event).
- **Manual smoke** (record in devlog):
  - Hover every DragValue, ComboBox, Button, Slider, Chip in
    each Inspector tool. Snapshot the tooltip count via grep:
    `grep -r 'on_hover_text' crates/barme-app/src/ — | wc -l`
    should report **>200**.
  - Click validation chip → `LintPanel` opens.
  - Click status-strip issue count → same panel opens.
  - Help (?) top-bar icon → cheat sheet opens.
  - All four paint-mask brushes show distinct ring colours on the
    2D paint viewport (visual check).
  - Minimap shows correct symmetry axes for the active mode.

- Rollup commit: STATUS UPDATEs in SRS / ROADMAP (U1 shipped);
  closing devlog log; "Sprint 20 = async build pipeline + in-app
  log surface" handoff note.

## Step 4 — Standing constraints

Same as prior sprints. `cargo fmt` + `cargo clippy --workspace
--all-targets -- -D warnings` + `cargo test --workspace` green at
each commit.

Tracing: `trace!` on `LintPanel` open/close; no other tracing
changes.

## Step 5 — Out of scope

- **Onboarding tour mode** — Sprint 22. Tooltips ship here; the
  tour reuses them.
- **`LintPanel` rule population** — Sprint 21 / C8.
- **Async build progress UI** — Sprint 20 / U3.
- **Toast queue / confirmation modals** — Sprint 31 / U4. Today
  the single `last_error` line stays; we replace it later.
- **Inspector layout refactor** — Sprint 27 / U5. Don't move
  widgets; just annotate them.
- **Help articles per tool** — Sprint 22.
- **Hover-popover-mode toggle** — Sprint 22 (`Ctrl+Shift+H`).
  This sprint ships passive hover-text only.

## Step 6 — Critical pitfalls (read twice)

1. **Tooltips are NOT documentation.** Each is 1-3 sentences.
   Long-form help lives in Sprint 22's help center. If you find
   yourself writing a paragraph, move it to a `// FIXME(sprint22):`
   placeholder for the help center and put a 1-liner in the
   tooltip.

2. **Don't change layout.** This sprint is annotative. If you
   discover an obvious widget that should move (e.g. the duplicate
   `height_scale` in both the header and the Water inspector),
   leave it for Sprint 27. Resist the refactor itch.

3. **`HelpId` enum exhaustiveness**: every interactive widget
   in the app must point to a `HelpId`. Use a per-Inspector
   `assert_total_help_coverage()` test or a `match` on each
   widget type that calls `help_text::help(id)` directly. If a
   widget has no `HelpId`, the test fails at compile time.

4. **Keyboard shortcuts in tooltips must be REAL.** If you cite
   `Ctrl+B` for Build, ensure `handle_keyboard` actually binds
   it. Audit before writing. The `cheat_sheet.rs` module is the
   source of truth for what's bound.

5. **Status-strip `"0 issues"`** — replace cleanly. Don't leave
   the old string commented out. The clickable region must be
   ALL of the issues count text, not just the icon.

6. **Validation-chip click affordance**: `egui` `Label`s aren't
   clickable by default. Use `.sense(Sense::click())` or wrap in
   `Button::frame(false)`. Test that the click event actually
   fires.

7. **Minimap symmetry rewrite**: the existing "unconditional
   vertical bisector" code has a mockup-history comment. Preserve
   the comment as a `// History:` annotation but replace the
   behaviour. Adding the new logic without removing the old draw
   call results in double-rendering.

8. **Don't break colour contrast.** The new `brush_ring_color`
   palette must remain distinguishable on the editor's dark
   theme. Use `Tokens::DARK`'s semantic names (`t.success`,
   `t.danger`, `t.accent`, `t.warn`) — do not hard-code RGB.

9. **F9 form (Sprint 18) tooltips**: the inspector_mapinfo form
   ships inline tooltips. After Sprint 18 lands, harvest those
   strings into `help_text.rs` and switch the inline calls to
   `help_text::help(...)`. Avoid drift.

10. **Hover-text on disabled widgets**: egui's `Response`
    silently swallows hover-text on disabled widgets. Use
    `.on_disabled_hover_text(...)` for cases like the greyed-out
    Launch combobox slot — surface "Sprint 32 ships F12 Launch
    in BAR" rather than no feedback.

## Step 7 — Exit criteria

- 5+ commits on `main` (catalogue, per-tool tooltips, validation
  click + status wiring, false-promise cleanup, rollup). Larger
  inspector chunks can be split further.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (U1 shipped; one UI sprint done,
  two to go).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- **Hover-text count >200** verified by grep (from <40 today).
- Smoke test:
  - Every widget across all 9 inspectors has a tooltip on hover.
  - Validation chip click + status-strip issue-count click both
    open the `LintPanel` stub.
  - Help (?) top-bar icon opens the cheat sheet.
  - Paint-mask brushes show distinct ring colours.
  - Minimap symmetry guide reflects the active mode.
- Final devlog log: summary + "Sprint 20 = async build pipeline +
  in-app log" handoff note.

Start by drafting `help_text.rs` — pin the `HelpId` enum and the
text strings BEFORE touching any inspector function. Then walk
the inspectors in order, swapping inline strings for
`help_text::help(...)` calls.
