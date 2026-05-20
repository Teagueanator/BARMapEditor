# Sprint 27 — Inspector consistency refactor + brush-card lift + sticky symmetry chip (U5)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 27** — a focused **refactor** sprint that cleans up
Inspector inconsistencies surfaced by the 2026-05-20 UX audit. The
Sprint 19 (tooltip) and Sprint 22 (onboarding) sprints landed help-
text and tour mode; this sprint **unifies the structural layout** of
all 9 tool inspectors so the user's mental model is consistent.

Specifically:

- All 9 inspectors follow a **header → sections → footer** pattern
  with consistent section accents.
- The four-card **brush selector** (Sculpt's Off/Raise/Lower/Smooth
  and PaintLayer's Reveal/Hide/Smooth/Fill) lifts into a shared
  widget — same colours, same hover behaviour.
- A **sticky symmetry+mapsize chip strip** appears at the top of
  every inspector body, so the user doesn't have to glance up at
  the chrome to remember "I'm in Quad mode".
- **Delete buttons** across all rows (metal spots, geo vents,
  features, start positions, ally groups, layers) use the same
  `widgets::icon_button(Icon::X)` + tooltip "Delete X — Ctrl+Z
  to restore".
- The duplicate `height_scale` (Inspector header AND Water inspector)
  collapses to ONE location with the better tooltip.
- The Sprint 6 / B7 procgen inspector's preset chips align with the
  Sprint 14 / C9 water inspector's preset chips — same widget shape.

After this sprint, the inspector feels like a single product, not
nine bespoke screens.

**Prerequisites:**
- Sprint 26 (water polish) MUST be ticked. The water inspector
  gains a "Polish" section that the refactor sees.
- Sprint 22 (onboarding + help center) MUST be ticked. The
  `help_text::HelpId` enum is the single source of truth for
  tooltips.
- Sprint 19 (tooltip pass) MUST be ticked.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/docs/research/ui/FINDINGS.md`
   — UX research with inspector inconsistency table (re-derived
   in the 2026-05-20 audit).
3. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/widgets.rs`
   — extend with `brush_card` widget. Centralise the existing
   `App::brush_card` helper (currently duplicated as a method).
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/main.rs::inspector_*`
   — all 9 inspector functions. The refactor touches each.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/help_text.rs`
   (Sprint 19) — every tooltip route through here.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/theme.rs`
   — `Tokens::DARK` semantic colours. The brush-card lift uses
   `t.success / t.danger / t.accent / t.warn`.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-27-inspector-consistency
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. `widgets::brush_card` — extract + unify

**Pattern today**: `inspector_sculpt` (main.rs:6974) and
`inspector_paint_layer` (main.rs:6904) each have a 4-card brush
selector with different colour palettes:
- Sculpt: Off=muted, Raise=success-green, Lower=danger-red,
  Smooth=accent-blue.
- PaintLayer: Reveal=success-green, Hide=danger-red,
  Smooth=accent-blue, Fill=warn-amber.

**Lift** (`crates/barme-app/src/ui/widgets.rs`):

```rust
pub struct BrushCard<'a> {
    pub label: &'a str,
    pub icon: Option<Icon>,
    pub ring_color: Color32,
    pub active: bool,
    pub hover_help: HelpId,  // Sprint 19's catalogue
}

pub fn brush_card(ui: &mut egui::Ui, card: BrushCard) -> Response;
```

Style: rectangular card, ~64×64 px, icon centred, label below in
small font, coloured ring around the icon when active. Hover →
faint highlight + tooltip from `help_text::help(card.hover_help)`.

Replace both `inspector_sculpt` and `inspector_paint_layer`'s
in-place card code with calls to `widgets::brush_card`.

### 2. Sticky symmetry + map-size chip strip

A horizontal strip rendered at the top of EVERY inspector body
(below the header strip, above the first section):

```
┌──────────────────────────────────────────────┐
│ [Symmetry: Quad]  [16×16 SMU]               │
├──────────────────────────────────────────────┤
│ Section 1                                    │
│ ...                                          │
└──────────────────────────────────────────────┘
```

Implementation: extend `widgets.rs` with `sticky_chip_strip(ui,
chips: &[ChipDesc])`. Each `ChipDesc` has `label`, `hover_text`,
optional `onclick` action.

For Sprint 27, two chips minimum:
- Symmetry chip: "None" / "Horizontal" / "Quad" / "Rotational
  N=4" / etc. Hover: full description from `help_text`. Click:
  opens the global Symmetry settings (or toggles cycle).
- Map size chip: "16×16 SMU" → "1025×1025 px". Hover: "1 SMU =
  64 elmos = 65 px (heightmap dim = 64·N+1). Map area = N·512
  elmos per side."

Render via `egui::TopBottomPanel::top` inside the inspector
function, or inline at the top of each `inspector_*` body. Pick
the cleaner option after writing the first one.

### 3. Standardised delete buttons

Audit every row delete button across:
- Metal spots (main.rs:6483).
- Geo vents (main.rs:6600).
- Features (main.rs:6821).
- Start positions (main.rs:7879).
- Ally groups (main.rs:7855).
- Layers (Sprint 17 / `layers_panel.rs:824`).

Each uses the same widget: `widgets::icon_button(Icon::X)` with
tooltip from `help_text::help(HelpId::DeleteRow(kind))`. The
help text is generated by row-kind:

```rust
HelpId::DeleteMetalSpotRow => "Delete metal spot. Ctrl+Z to restore.",
HelpId::DeleteAllyGroupRow => "Delete ally group AND all start positions inside it. Ctrl+Z to restore.",
// ... and so on
```

**Visual treatment**: every delete button is the same size (16
px square), same colour (`t.muted` default, `t.danger` on hover).
Replace any text "delete group" buttons with the icon.

### 4. Section pattern enforcement

Define the canonical structure:

```
SECTION_1 (accent: true)         ← always the primary section
SECTION_2 (accent: false)
SECTION_3 (accent: false)
[Sticky footer: action buttons if any, e.g. "+ Add"]
```

Audit each inspector and ensure:
- Exactly ONE section uses `accent: true` (the "main" section
  for that tool).
- Remaining sections use `accent: false`.
- Section titles are SHOUT_CASE via `widgets.rs:52`.
- The PaintLayer inspector (currently in `layers_panel.rs`) ALSO
  follows this — the Layers section is the accent one.

### 5. Resolve duplicate widgets

**`height_scale` duplicate**:
- Inspector Header (main.rs:6309) shows it.
- Water Inspector (main.rs:7186-7228) shows it AND `min_height`
  with better tooltips.

**Resolution**: keep `height_scale` + `min_height` in the
**Inspector Header** as the canonical location. Remove from Water
Inspector. Move the Water tooltip strings into `help_text.rs` so
they're attached to the canonical widgets.

**`extractor_radius` duplicate**: Metal inspector has the
per-project default; F9 form (Sprint 18) has the same field
under Map tab. F9 form acts as the schema view; Metal inspector
acts as the active editing path. Both edit the same field; no
change needed beyond documenting.

### 6. Inspector function rewrite

Each `inspector_*` function should follow the same skeleton:

```rust
fn inspector_metal_spots(&mut self, ui: &mut egui::Ui) {
    // 1. Header strip (project name + chips) — from inspector_header
    self.inspector_header(ui);
    // 2. Sticky symmetry + mapsize chips
    widgets::sticky_chip_strip(ui, &[
        ChipDesc { label: self.symmetry_label(), hover_text: ..., onclick: None },
        ChipDesc { label: format!("{}×{} SMU", smu_x, smu_z), hover_text: ..., onclick: None },
    ]);
    // 3. Primary section (accent)
    widgets::section(ui, "METAL SPOTS", true, Some(help::MetalSpotsSection), |ui| {
        // ... table of spots
        // ... + Add button at bottom
    });
    // 4. Secondary section
    widgets::section(ui, "DEFAULTS", false, Some(help::MetalDefaults), |ui| {
        // ... extractor_radius drag
    });
}
```

Repeat for Sculpt, StartPositions, GeoFeatures, Feature, Water,
PaintLayer (in layers_panel.rs), Procgen, Select. **One commit
per inspector** to keep the diff bisectable.

### 7. Tests + smoke run + rollup

- **Visual smoke test**: open each tool; verify:
  - One accent section per inspector.
  - Sticky chip strip visible.
  - Delete buttons in rows are uniform.
  - Brush cards (Sculpt + PaintLayer) use the same widget.
  - No duplicate height_scale field.
- **Compile-time pattern check**: an `assert_inspector_pattern`
  test that calls each inspector function in a fake `ui` and
  verifies it emits exactly one accent section + the sticky
  strip. Use egui's `Context::run` headlessly.
- **Hover-text count test** (Sprint 19) — still passes; the
  refactor doesn't lose tooltips.
- **Rollup commit**: STATUS UPDATEs in SRS / ROADMAP (U5 done,
  three UI polish sprints + one consistency refactor done);
  closing devlog log; "Sprint 28 = atmosphere + fog" handoff.

## Step 4 — Standing constraints

Same as prior sprints. **No new ADR** — this is a refactor; the
existing ADR-030 (Phase 3 layout shell) and ADR-035 (widget
contract) cover the pattern.

## Step 5 — Out of scope

- **Wholesale redesign of the inspector** — Sprint 27 unifies
  the existing pattern, doesn't redesign it.
- **Per-tool empty states with illustrated cards** — would be
  Stage-2 polish.
- **Touch / pen support** — out of scope.
- **Resizable Inspector with multi-column layout** — out of
  scope.
- **Accessibility (keyboard focus order)** — Stage-2 polish.
- **Theme toggle (light/dark)** — F21, deferred to a future
  sprint.

## Step 6 — Critical pitfalls (read twice)

1. **Don't lose hover-text during refactor**. Sprint 19's
   tooltip pass put ~80+ tooltips on widgets; refactoring
   without preserving them is a regression. Run the hover-text
   count grep before and after — number must not decrease.

2. **Brush card lift must preserve keyboard accelerators**.
   Sculpt has `R / L / S` for Raise/Lower/Smooth bindings via
   `handle_keyboard`. PaintLayer's mask brushes don't have
   keyboard bindings (Sprint 17 didn't add them). Don't add
   them here either — Sprint 27 is a refactor, not feature work.

3. **Sticky chip strip rendering order**: it goes BETWEEN the
   header strip and the first section, NOT inside the
   `egui::ScrollArea`. The chips should stay visible when the
   inspector body scrolls.

4. **`height_scale` removal from Water Inspector**: must
   preserve the better tooltips. Migrate the tooltip strings
   into `help_text.rs` BEFORE removing the widget.

5. **Don't break undo**. Every mutation must still flow through
   `ProjectDiff`. The refactor only changes widget layout, not
   data paths.

6. **PaintLayer's inspector lives in `layers_panel.rs`** — not
   in `inspector_paint_layer`. The refactor touches both
   files to apply the pattern. Be careful: `layers_panel.rs`
   is rendered inside a `Tool::PaintLayer` branch in
   `main.rs::central` panel, not in the Inspector right strip.

7. **Section accent counts**: write a debug-mode assertion that
   panics if a single tool emits >1 accent section. Easier to
   catch in dev than in code review.

8. **Don't refactor without tests**. Each inspector commit
   should have a pre/post smoke test (snapshot via egui's
   `bench_print` or `Context::end_frame`).

9. **Delete buttons in `layers_panel.rs`**: the layer
   delete uses `×` text; replace with `icon_button(Icon::X)`.
   The lock chip (`Icon::Lock`) and DNTS channel chip stay as
   they are.

10. **Don't churn `next_steps.rs` / `intro.rs`**: those are
    onboarding-only surfaces, not inspectors. Leave them alone.

## Step 7 — Exit criteria

- 12+ commits on `main`: widget extraction (1), sticky chip strip
  (1), delete-button standardisation (1), section pattern
  enforcement per inspector (9), rollup (1).
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (U5 done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Hover-text count >= 200 (same as Sprint 19 baseline).
- Smoke test:
  - Cycle through all 9 tools — visual rhythm consistent.
  - Symmetry chip + mapsize chip visible at top of each
    inspector.
  - Delete buttons across all row types are identical icons.
  - Brush cards in Sculpt and PaintLayer look identical except
    for label/colour.
  - No duplicate height_scale field.
- Final devlog: summary + "Sprint 28 = atmosphere + fog"
  handoff.

Start by extracting `brush_card` into `widgets.rs` and replacing
the two existing call sites. That establishes the refactor
pattern; the per-inspector chunks follow mechanically.
