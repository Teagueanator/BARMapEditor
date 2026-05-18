# Research prompt — UX design for an intuitive 3D map editor

**Use:** Paste the section below verbatim into a fresh Claude deep-research
session.

**Expected output:** an opinionated design-recommendation document
identifying the top ~10 UX failures in our current editor and prescribing
specific fixes drawn from industry precedent (Blender sculpt, World
Machine, Houdini terrain, Unity Terrain, PA System Designer, WorldEdit,
Mapping/SourceForge-era editors, modern game-engine tooling). We will
adopt the accepted recommendations as a Phase 3 ADR (likely ADR-028 or
later) and a wave of focused commits.

---

## Prompt (copy from here)

You are the senior UX designer for *BAR Map Editor*, an open-source desktop
3D heightmap + map-asset editor for Beyond All Reason (Recoil engine, a
fork of Spring RTS). It is a single-binary Rust + egui + wgpu application
that produces playable `.sd7` archives. The current build is functional
but its users report **the editor is unintuitive**. Your job is to
diagnose *why* and prescribe specific concrete fixes — both in layout and
in interaction model.

This is not a code-writing exercise. The output is a design document the
implementation team will execute against.

### Current state of the editor

**Window layout:**
- Top menu bar: `File` | `Edit` | `Build`
- Left side panel (vertical scroll, always visible, ~280 px wide), with
  sections stacked top-to-bottom:
  1. **Project** — name (text), size (smu_x × smu_z DragValues), open file
  2. **Heightmap** — current path, dims, min/max sample, validation status
  3. **Render** — Max height (elmos) DragValue
  4. **Tool** — radio: Sculpt | Start positions
  5. **Sculpt** — brush picker (Off/Raise/Lower/Smooth), radius (elmos),
     strength (0..=1), symmetry dropdown (None/H/V/Quad/Diag\/Diag/Rot)
     + rotational fold (2..=12), camera readout (yaw/pitch/dist)
  6. **Start positions** — placement hint, list of placed teams with
     delete buttons, clear-all
  7. **Generate from formula** — expression text field, domain radio
     (`[0,1]` / `[-1,1]`), preset dropdown, Apply button
  8. **Build & Install** — Build button, install status
- Central viewport: 3D terrain preview rendered via wgpu. Orbit camera.
- A modal `egui::Window` "New project" wizard pops up on launch with:
  name, smu_x × smu_z, symmetry preset, biome preset, max-height.

**Interactions:**
- *Sculpt mode:* LMB drag = paint, RMB drag = orbit camera, scroll = zoom.
- *Start positions mode:* LMB click empty = place team, LMB drag existing
  marker = move, RMB click on marker = delete, RMB drag = orbit.
- *Camera controls* (when no tool active): LMB drag = orbit, scroll = zoom.
- Ctrl-Z / Ctrl-Shift-Z = undo / redo (heightmap-only edits).

**What the editor produces:** an `.sd7` archive that BAR loads as a
playable map. The user's mental model is "design a map that plays
well" — terrain shape, team start spots, eventually textures and metal
spots. Comparable reference apps: **Blender's sculpt mode**, **World
Machine**, **Houdini Terrain**, **Unity Terrain tools**, **Planetary
Annihilation's system designer**, **WorldEdit** (Minecraft), **Spring's
historical SpringMapEdit / EasyMapEdit**.

### Known frustrations from users

These are the verbatim complaints we have so far:

1. *"It's very very unintuitive."* — repeated, no specifics.
2. The Start-Position editor only supports one team per symmetry image.
   Users want 8 spots per side (8v8 team game), or 3 sides with 8 spots
   each (3-way FFA). The current model is wrong for real BAR maps.
3. Symmetry settings live in the *Sculpt* section but also affect Start
   Positions — users can't see that connection.
4. After placing positions, switching to Sculpt brush mode silently
   leaves the position markers visible but unclickable; users don't
   understand why their clicks no longer move markers.
5. No on-canvas brush size preview (no circle under the cursor showing
   what radius the next stamp will cover).
6. The "Generate from formula" panel exposes raw math syntax
   (`max(0, 1 - math::sqrt(x*x + z*z))`) — users mistype it
   (`x*2x` instead of `x*x*x`) and only see the error after Apply.
7. The build / install workflow is buried at the bottom of the side
   panel; users don't realise their first move after sculpting should
   be Build & Install.
8. Camera controls (orbit / zoom / pan) are entirely discoverable by
   accident. No on-screen affordance hints "drag here to rotate" or
   "scroll to zoom."

### Constraints on your recommendations

1. **Stay within egui's idiom.** Hard requirement: the implementation is
   `egui` (immediate-mode). Recommend layouts that egui can build, not
   patterns that require retained-mode trees. Acceptable widgets: panels,
   windows, modals, painters (for custom overlays), drag-and-drop. *Not*
   acceptable: dockable panes with arbitrary user-rearrangement (egui's
   `Dock` is third-party and unstable as of 0.33), HTML-style flexbox.
2. **Keep the binary single-window.** No multi-window editors. The
   central 3D viewport is non-negotiable; everything else docks around it.
3. **Don't trade off determinism or observability.** Every UI action
   should map to a discoverable command (we already have tracing on
   every input). Hidden gestures fail this bar.
4. **The user persona is a BAR community mapper**, not a Blender power
   user. Assume zero experience with terrain DCC. Common path: install
   the editor → click around to see what happens → produce a playable
   map within ~30 minutes.

### Required output: a UX design document

Structure it like this:

```markdown
# UX Recommendations for BAR Map Editor (Phase 3)

## 1. Layout — top-level window structure

[Diagram and prose. Recommend a layout for the left panel, central
viewport, any new bottom strip / right inspector / floating gizmos.
Explain what moves where, what stays, what gets removed. Reference
specific industry precedents — Blender's tool tabs, Houdini's
parameter pane, Unity's Inspector.]

## 2. Tool-mode model — Sculpt vs Place vs (future) Paint vs Select

[How should the user switch tools? A vertical tool-bar of icons on
the *left edge* (Blender / Krita / Photoshop), tabbed sections in
the side panel (Houdini), or the current radio? What does the
hierarchy look like once Phase 3 adds Splat and Phase 4 adds Metal /
Geo / Features? Show the layout at Phase 5 maturity.]

## 3. Symmetry as a global mode

[Symmetry affects sculpting AND start-position placement AND eventually
splat painting AND metal placement. It should be a *global* property
of the current edit session, surfaced in a way the user can never
miss. Recommend: persistent badge in the toolbar? Coloured outline on
the viewport when active? On-canvas axis lines showing the symmetry
line / center? Reference Blender's symmetry indicator and the way
2D pixel-art editors show mirror axes.]

## 4. Start-position editor redesign

[The biggest broken feature. Address all of:
- Multiple positions per side (N teams in one symmetry image)
- Allyteam grouping (3 sides × 8 spots arrangement)
- Drag-paint a line of spots
- Per-position colour override + per-allyteam team colour
- Visible feedback: hover a position in the side-panel list, it
  pulses on the map; click-select highlights it
- Multi-select + bulk-delete + bulk-move]

## 5. On-canvas feedback

[What should the central viewport overlay when each tool is active?
Brush size circle, symmetry axes, hover targets, snap guides, ruler /
elmo grid, etc. Reference World Machine's brush preview and Houdini's
viewport handles.]

## 6. Discoverability of camera controls

[Drag = orbit, scroll = zoom, RMB = pan(?). Recommend on-screen
affordances: corner gizmo (Blender's axis gizmo), first-launch hint
overlay, `?` key for keyboard cheat-sheet. Specify which.]

## 7. Build / preview workflow

[The Build & Install button is buried. Most apps put the "run / build /
preview" affordance in the top bar (web IDEs, game engines). Recommend
a top-bar action button next to the menus, plus a status strip
showing build state, validation warnings ("you have no start
positions — map will use 25/75 default"), and a one-click "preview in
BAR" once F12 lands.]

## 8. Procgen UX (the `f(x, z)` panel)

[Live parse on every keystroke (don't wait for Apply). Show parse
errors as a red underline + tooltip with the parser's source-chain.
Add a "preview" thumbnail of the expression result before committing.
Promote the preset dropdown above the freeform expression — most
users will pick a preset, never touch the math.]

## 9. The first 30 seconds

[Map out the new-user flow: install → launch → first map shipped.
What should appear when the wizard closes? An empty greyscale
terrain with a hint overlay? A pre-populated 16×16 with default
biome and 2 start positions? Recommend the *demo state* that proves
the editor works without the user reading any docs.]

## 10. Reference inventory

[A 1-row-each table of editors you cited, what specifically each
does well, and a one-line link to a screenshot or doc page that
illustrates it. We need this so the implementation team can mirror
the references without re-doing the literature scan.]
```

### Reference editors to draw from

Concrete editors to inspect and cite. For each, locate primary
documentation / screenshots (not just blog posts) and cite the
specific page that supports your recommendation.

- **Blender Sculpt mode** — the gold standard for brush-driven 3D
  editing. Note its symmetry indicator, brush curves, and tool-tab
  layout on the left.
- **Blender's Terrain plugin (ANT Landscape, A.N.T. Landscape)** — for
  procgen UX in immediate-mode-ish settings.
- **World Machine 4** — node-graph terrain authoring. Note its
  layered approach (heightfield + masks + texturing).
- **Houdini Terrain (Heightfield SOPs)** — node-based but with strong
  viewport-handle feedback.
- **Unity Terrain Editor** — the closest comparable for "terrain
  inside a game engine." Note its tool-tab strip.
- **Planetary Annihilation System Designer** — the SRS explicitly
  cites this as the target UX feel. Find a video / docs and call out
  what specifically works.
- **WorldEdit (Bukkit / Forge)** — text-command-driven, but the way
  it stages and previews region operations is instructive for the
  symmetry / multi-team flows.
- **GIMP / Krita** — for tool-tab UI patterns and on-canvas brush
  preview.
- **Spring's historical SpringMapEdit and EasyMapEdit** — even though
  they're abandoned, the BAR community knows them; identify what
  worked and what made them get abandoned.
- **JandoDev/bar-editor** (`github.com/Jandodev/bar-editor`) — direct
  competitor; inspect its UX choices and note where we should
  converge / diverge.

### Process constraints

- **Cite primary sources.** Screenshots, official docs, repo README.
  Don't synthesise without a source.
- **Recommend, don't survey.** Don't end every section with "the team
  should decide." Pick a specific design and defend it.
- **One document, ≤ 3000 words.** Crisp.
- **Section 1 (layout) is the single most important.** If you only
  do one section well, do that one.

---

## What we'll do with the output

1. Read the document end-to-end.
2. For each of sections 1–9, decide accept / modify / reject.
3. Commit the accepted recommendations as Phase 3 ADRs — likely one
   for layout (ADR-028), one for the start-position redesign
   (ADR-029), and small follow-ups for individual sections.
4. Implementation begins with the layout refactor (biggest visible
   change). Other sections become tasks under Phase 3 in
   `devlog/stage-1-mvp/goals.md`.
