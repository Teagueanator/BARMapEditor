# UX Recommendations for BAR Map Editor (Phase 3)

**Bottom line:** The current editor's single-list left panel collapses six unrelated jobs (project, validation, tool choice, tool params, object editing, build) into one scroll-bar, which is the root cause of every reported frustration. Phase 3 must split the window into a four-zone shell — left **tool strip**, left **inspector**, central **viewport**, top **action bar**, bottom **status strip** — promote symmetry to a global session mode visible in the action bar and as a viewport overlay, and rebuild the start-position editor around allyteams. These five moves resolve frustrations #1, #3, #4, #5, #7, and #8 at once, and Section 9 specifies a pre-populated demo state that resolves #1 (the "very very unintuitive" complaint) on first launch.

---

## 1. Layout — top-level window structure

The current layout is a single 280 px `SidePanel::left` containing eight stacked, scrolling sections, plus a `TopBottomPanel::top` menu and a `CentralPanel` viewport. That model is the cause of frustrations #1, #3, #4, and #7. It must be replaced with the following five-zone shell, all of which is achievable with egui's standard `TopBottomPanel`, `SidePanel`, `CentralPanel`, and `Painter` primitives in the documented add-order — "The order in which you add panels matter! The first panel you add will always be the outermost, and the last you add will always be the innermost. ⚠ Always add any `CentralPanel` last." (https://docs.rs/egui/latest/egui/containers/panel/).

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  TOP ACTION BAR  (TopBottomPanel::top, 36 px)                               │
│  File  Edit  Build │ ◐ Sculpt ▾ │ ⟦Sym: Quad·Rot2⟧ │ … │ ▶ Build & Install │
├──┬────────────────────────────────────────────────────────────┬────────────┤
│T │                                                            │ INSPECTOR  │
│O │                                                            │ (right     │
│O │              CENTRAL VIEWPORT (CentralPanel)               │  SidePanel,│
│L │              wgpu terrain + egui Painter overlay           │  resizable,│
│  │              ┌────┐                                        │  300 px)   │
│S │              │ ⟲X │  ← nav gizmo (top-right corner)         │            │
│T │              └────┘                                        │ • Project  │
│R │                                                            │ • Heightm. │
│I │              ◯ brush preview (Painter circle)              │ • Tool ops │
│P │              ╎ symmetry axes (Painter dashed)              │   (context)│
│  │                                                            │ • Validate │
│40│                                                            │            │
├──┴────────────────────────────────────────────────────────────┴────────────┤
│  STATUS STRIP  (TopBottomPanel::bottom, 24 px)                              │
│  Cam: 1024,800,950 │ 256×256 smu │ ⚠ 0 start positions  │  Build: idle     │
└─────────────────────────────────────────────────────────────────────────────┘
```

**What MOVES:**
- **Tool radio → left tool strip.** Vertical 40-px-wide icon strip pinned to the window's left edge. This mirrors Blender's Sculpt-mode toolbar — "The amount of tools in sculpt mode is very extensive. This is an overview of all of them, categorized by their general functions." (https://docs.blender.org/manual/en/latest/sculpt_paint/sculpting/toolbar.html) — and Krita's Toolbox docker, which is part of the default workspace layout (https://docs.krita.org/en/reference_manual/resource_management/resource_workspace.html). In egui this is a non-resizable `SidePanel::left` of fixed width containing `SelectableLabel` widgets stacked vertically.
- **Tool-specific parameters (Sculpt brush, Start Positions list, Procgen form) → right Inspector.** A resizable `SidePanel::right` whose contents are driven by the active tool — exactly the pattern Unity uses: "Select the paintbrush icon to access painting tools, which allow you to modify the Terrain. Use the cursor to sculpt the height of the Terrain, or paint texture onto the Terrain. Choose from several built-in Brush shapes, or define your own Brush using a texture. You can also change the size and opacity (the strength of the applied effect) of the Brush." (https://docs.unity3d.com/6000.1/Documentation/Manual/terrain-UsingTerrains.html).
- **Build & Install → top action bar.** A prominent green ▶ button at the right edge of the top bar, resolving frustration #7. Unity Play, Unreal Play-in-Editor, and every web IDE converge on this position.
- **Project name / heightmap path / validation → top of the right Inspector, always visible.** These are session metadata, not a tool, and need to be checkable at a glance without scrolling.

**What STAYS:**
- The top menu bar (`File | Edit | Build`) and the central wgpu viewport (`CentralPanel`).
- The new-project wizard `egui::Window` modal on launch.
- Ctrl-Z / Ctrl-Shift-Z and the existing tracing on every input.

**What gets REMOVED:**
- The single scrolling left column with eight unrelated sections.
- The "Render: Max height elmos" mini-section as a top-level item — it moves into a collapsing header inside the Inspector under **Heightmap**.
- The standalone "Camera readout" inside the Sculpt section — it moves to the bottom status strip.

**egui feasibility.** All five zones are concrete `TopBottomPanel`/`SidePanel`/`CentralPanel` calls in the order required by egui (top, bottom, left tool strip, right inspector, then central last; https://docs.rs/egui/latest/egui/containers/panel/). The viewport overlays in §5 are drawn with `ui.painter()` on top of the wgpu texture. **No third-party dock crate, no flexbox, no retained tree.**

**Why this specific design.** It separates *what tool am I in?* (left strip, always visible), *what does this tool need from me right now?* (right inspector, context-driven), *what is the model showing?* (central viewport), *what is the global session state?* (top bar — symmetry, file, build), and *what happened/what's wrong?* (bottom strip — camera, validation, build state). Every frustration in the brief maps to a specific zone whose job is to fix it.

---

## 2. Tool-mode model — Sculpt vs Place vs Paint vs Select

**Recommendation: a vertical icon toolbar on the left edge, single-selection, exclusive, with a one-letter accelerator under each icon.** This is the Blender Sculpt-mode pattern (https://docs.blender.org/manual/en/latest/sculpt_paint/sculpting/toolbar.html), Krita's default Toolbox arrangement, and Photoshop's tools palette. It scales to ten tools without redesign and gives every tool a stable visual home, which immediate-mode radio buttons inside a scrolling panel cannot.

The radio-in-side-panel pattern fails at Phase 5 maturity because adding Splat-paint, Metal placement, Geo features, and Selection means ten radio buttons buried under unrelated controls. Houdini's tabbed parameter pane is rejected because BAR is not node-based and tabs reintroduce the "where did my option go?" problem.

**Phase 5 left tool strip (top to bottom):**

```
 ┌──┐
 │↺ │  Select / orbit-only (no edit)         [Q]
 │✎ │  Sculpt heightmap                       [B]
 │⚑ │  Start positions                        [S]
 │▦ │  Splat paint (texture layers)           [T]
 │◆ │  Metal spots                            [M]
 │🌲 │  Geo features (rocks, trees)           [F]
 │ƒ │  Procgen / formula                      [G]
 └──┘
```

Tool switches are tracked by the existing tracing layer. The active tool gets a highlighted background and renders its parameter set in the right Inspector below the always-visible Project/Heightmap headers. Frustration #4 (markers visible but unclickable in Sculpt mode) is addressed by the rule in §4: start-position markers stay visible across tools but only consume mouse input under the Start Positions tool. Unity follows the same convention: tool overlays only become interactive "When a Terrain GameObject is selected" with the appropriate paint tool active (https://docs.unity3d.com/6000.1/Documentation/Manual/terrain-UsingTerrains.html).

---

## 3. Symmetry as a global mode

Symmetry is not a sculpt parameter; it is a session property that governs every spatial edit — heightmap brushes, start positions, splat paint, metal placement, feature scatter. **Promote it to the top action bar and render a viewport overlay whenever it is on.**

**Top-bar widget (right of tool selector, always visible):**

```
⟦ Sym: Quad · Rot×2 ▾ ⟧
```

Click opens a small `egui::Window` popover with the existing radio set (None / H / V / Quad / Diag / Diag2 / Rotational) and the 2..=12 fold spinner. This mirrors Blender's symmetry placement: in Sculpt mode, "These three buttons allow you to block any modification/deformation of your model along selected local axes, while you are sculpting it… These settings allow for radial symmetry in the desired axes. The number determines how many times the stroke will be repeated within 360 degrees around the central axes." (https://docs.blender.org/manual/en/latest/sculpt_paint/sculpting/tool_settings/symmetry.html).

**Viewport overlay (drawn with `ui.painter()` whenever symmetry ≠ None):**
- A dashed white line for each mirror axis, drawn from heightmap edge to heightmap edge, intersecting at the map center.
- For Rotational(N), N evenly-spaced spokes from the center.
- A small ⟦SYM⟧ chip in the top-right of the viewport (next to the nav gizmo) that pulses for ~400 ms when the mode changes, so the user notices the change.

This is the 2D pixel-art convention: Aseprite renders a draggable on-canvas handle for each symmetry axis — "When you enable one symmetry axis (e.g. horizontal symmetry/vertical axis) you can drag-and-drop on-screen handles to configure the position of the axis: Then just drawing will paint in both sides of the image" (https://www.aseprite.org/docs/symmetry/). We adopt the visible-axis half of that idea, but **lock axes to the map center** — BAR maps are always symmetric about the geometric center, and a movable axis would break engine assumptions about start-position pairing.

The Sculpt section's symmetry controls are **removed** from the right Inspector once the top-bar widget exists; this resolves frustration #3 (symmetry hidden inside Sculpt but affecting Start Positions).

---

## 4. Start-position editor redesign

The current model — "one team per symmetry image" — is wrong for BAR. The Spring engine's `mapinfo.lua` schema encodes start positions as a 0-indexed `teams` table — `teams = { [0] = { startPos = { x = 2033, z = 852 } }, [1] = { startPos = { x = 10134, z = 852 } }, ... }` per the Spring wiki Mapdev:mapinfo.lua page (https://springrts.com/wiki/Mapdev:mapinfo.lua) — and the BAR FFA-gadget README states verbatim: "the start positions defined from mapinfo.lua are a plain list, which is not flexible enough to define multiple layouts of start positions depending on the number of contestants (e.g. one specific layout for 5-way, another one for 7-way)" (https://github.com/beyond-all-reason/Beyond-All-Reason/blob/master/luarules/configs/ffa_startpoints/README.md). The Recoil/Spring engine itself does not hard-cap the team table; the editor must enforce sane structure.

**New data model in the editor.** Two-level grouping:

```
AllyTeam 0  (red)            ← group, has team colour
  ├─ Pos 0   (1280, 840)
  ├─ Pos 1   (1280, 920)    ← drag-painted line of 8
  └─ Pos 7   (1280, 1400)
AllyTeam 1  (blue)
  ├─ Pos 8   (mirror of Pos 0 — derived, greyed)
  ...
AllyTeam 2  (green)          ← third side for 3-way FFA
  └─ ...
```

Allyteams are first-class. The right Inspector for the Start Positions tool shows a tree:
- Each allyteam row: colour swatch (click to override), label, count, ✕ delete-allyteam button.
- Each position row: index, world coords, optional per-position colour override, ✕ delete, ☐ multi-select checkbox.
- Hover a row → the corresponding marker on the canvas pulses (Painter draws a thicker ring at 2 Hz for 1 s).
- Click a row → marker highlights (yellow outline).
- Shift-click or Ctrl-click → multi-select; bulk delete and bulk-move-by-offset buttons appear.

**Canvas interaction (Start Positions tool active):**
- LMB click on empty terrain → add position to the **currently selected allyteam**; symmetry images are computed and added to the partner allyteams automatically.
- LMB drag from empty terrain → drag-paint a line; the editor samples N evenly-spaced points along the drag (N from a spinbox in the Inspector, default 8 — the canonical 8v8 case).
- LMB drag on existing marker → move it (and its symmetry images, derived live).
- RMB click on marker → delete (and its symmetry images).
- "Add Allyteam" button in Inspector → adds a third or fourth side for 3-way / 4-way FFA.

**Cross-tool visibility (resolves frustration #4).** Markers render in every tool, but only consume mouse input under the Start Positions tool. In all other tools they are drawn at 50 % opacity with no hover tint, and hovering a marker shows the tooltip *"Switch to Start Positions (S) to edit."* The cursor does not change. This is the same convention Unity uses for Terrain tool overlays — the brush cursor only appears when the relevant tool is active (https://docs.unity3d.com/6000.1/Documentation/Manual/terrain-UsingTerrains.html).

**Per-allyteam colour** uses BAR's faction palette as defaults (Armada blue, Cortex red, Legion green); per-position override is a `Color32` `color_edit_button_srgba` in the Inspector row.

---

## 5. On-canvas feedback

The central viewport is an `egui::CentralPanel` containing the wgpu terrain texture. **Overlays are drawn in the same panel via `ui.painter()` after the wgpu texture, before egui flushes the frame.** All overlays are tool-conditional.

**Always on (every tool):**
- **Nav gizmo** (Section 6) — top-right corner.
- **Elmo ruler** — a thin two-axis ruler along the left and bottom edges of the viewport, ticks every 512 elmos, labeled every 1024. World Machine's Leftside View documents an equivalent overlay: "Contour Lines: Show contour lines on the terrain, much like a traditional topographic map. Very helpful in understanding the shape of the terrain in areas of low contrast." (https://help.world-machine.com/topic/manual-4-side-panel/).

**Sculpt tool:**
- **Brush ring** at the cursor: a circle whose radius is the brush radius projected to world units, drawn with `painter.circle_stroke`. Inner faint ring at radius × falloff. Colour varies by mode (Raise = green, Lower = red, Smooth = blue, Off = grey). This resolves frustration #5. Unity's documented behaviour is identical: "If you select the leftmost tool on the bar (Raise/Lower Terrain) and move the mouse over the terrain in the scene view, you will see a cursor resembling a spotlight on the surface." (https://docs.unity3d.com/530/Documentation/Manual/terrain-UsingTerrains.html). Houdini's HeightField Draw Mask is invoked the same way — "Add a HeightField Draw Mask in the network editor, select it, and in the viewer click the Handles tool or press Enter. Draw on the terrain to enclose filled areas in the mask." (https://www.sidefx.com/docs/houdini/nodes/sop/heightfield_drawmask.html).
- **Symmetry axes** (if symmetry on) — dashed white lines from edge to edge.
- **Mirror brush ghosts** — at each symmetry image, a faint version of the brush ring so the user sees where the stroke will also land.

**Start Positions tool:**
- All markers at full opacity, hover → 1.2× scale + tooltip showing world coords and allyteam.
- Drag-paint preview: a dashed line and N ghost markers along it.
- Snap guides: when a placed position falls within 64 elmos of a metal spot or another position, draw a snap-guide line.

**Procgen tool:** preview thumbnail on top of viewport (Section 8).

---

## 6. Discoverability of camera controls

**Recommendation: three concrete affordances. Ship them in this order.**

1. **Nav gizmo in the top-right of the viewport.** A 64 px coloured-axis compass widget drawn by `ui.painter()` and consuming pointer events in its small rect. Click an axis sphere → snap to that orthographic view; drag the gizmo → orbit. The Blender Manual documents this affordance: under *Viewport Gizmos* the "navigation gizmo" is one of the top-level enables, and gizmos "have three color-coded axes: X (red), Y (green), and Z (blue). You can drag an axis with LMB to transform along it." (https://docs.blender.org/manual/en/latest/editors/3dview/display/gizmo.html). The gizmo doubles as a discoverability beacon — its presence tells new users that orbit exists at all.

2. **First-launch hint overlay.** A semi-transparent `egui::Window` (no title bar, drawn over the viewport) that appears on first run, listing the three camera gestures with a small diagram each: *RMB drag = orbit · Scroll = zoom · Middle drag = pan.* Dismissed with "Got it" and never shown again. Stored in the editor's config TOML keyed by editor version, so a major release can replay it once.

3. **`?` key opens a keyboard cheat-sheet modal.** An `egui::Window` listing every binding in a two-column table, grouped by tool. The existing tracing layer already records every input, so the table can be auto-generated from the keymap definition. Krita and Blender both ship equivalents; this is the cheapest discoverability win.

**Bindings standardised at:** Sculpt tool — LMB paint, RMB orbit, Middle pan, scroll zoom. Start Positions / Splat / Metal — LMB primary, RMB orbit, Middle pan. Select / no-tool — LMB orbit, Middle pan, scroll zoom. RMB-as-pan is rejected because it collides with start-position deletion.

---

## 7. Build / preview workflow

The Build & Install button moves out of the bottom of the side panel and into the **top action bar** as a labelled green ▶ button at the right edge, with a dropdown caret for advanced options (Build only / Build + Install / Build + Install + Launch). This mirrors every contemporary game engine and web IDE (Unity Play, Unreal Play-in-Editor, VS Code Run).

**Status strip (bottom `TopBottomPanel::bottom`, 24 px):**

```
Cam: 1024,800,950 │ Map: 16×16 smu │ ⚠ 0 start positions (engine will spawn at map corner) │ ✔ Heightmap valid │ Build: idle
```

The strip is always visible and always reflects the last validation pass. Validation runs on every meaningful state change (debounced) and the chips are clickable: clicking the "⚠ 0 start positions" chip selects the Start Positions tool and opens the Inspector. The fallback-wording is grounded in BAR's documented engine behaviour — the BAR FFA-gadget README notes "some maps do not even define enough start positions to allow for all players to spawn" (https://github.com/beyond-all-reason/Beyond-All-Reason/blob/master/luarules/configs/ffa_startpoints/README.md), and the BAR map checklist requires "Add fixed start positions (teams{} should be set properly for max. number of start positions + teams)" (https://www.beyondallreason.info/guide/map-checklist).

**One-click "Preview in BAR"** (Phase 5+, when F12-style hotload lands): the ▶ dropdown gains a "Build & launch in BAR" item that spawns `BAR.exe` with the freshly-built `.sd7` and a development modoption. Until then the dropdown contains only "Build" and "Build + Install".

---

## 8. Procgen UX (the f(x,z) panel)

The current "Apply" gate is the bug. Replace it with:

1. **Live parse on every keystroke.** Use `Response::changed()` on the `TextEdit` and reparse synchronously (the existing parser is fast enough for typical expressions). On parse error, the `TextEdit` is drawn with a red underline (set `TextEdit::frame(true)` then override the stroke in the response rect via `ui.painter()`), and the error tooltip shows the parser's source-chain — column number, token, expected. WorldEdit's `//gen` command uses the same expression-parser pattern — "This uses the expression parser… `-c` Shift the origin to the center of your selection, with one block equaling one unit" (https://github.com/EngineHub/WorldEditDocs/blob/master/source/usage/generation.rst) — but defers parsing to command submission; the lesson from frustration #6 is to never defer.

2. **Promote preset dropdown ABOVE the expression field.** The Inspector layout for the Procgen tool becomes, top to bottom: Preset (dropdown) → "Custom expression" expander → Domain radio → live preview thumbnail (256 × 256 px, drawn into a `TextureHandle` and shown with `ui.image`) → Apply. Blender's A.N.T. Landscape addon follows the same "preset first, freeform second" hierarchy — "After creating your landscape mesh there are three main areas in the Adjust Last Operation panel to design your mesh. Main Settings… Noise Settings… Displace Settings" with preset selection at the top (https://docs.blender.org/manual/en/4.1/addons/add_mesh/ant_landscape.html).

3. **Preview thumbnail of the expression's output**, recomputed on a 50 ms debounce after parse success. Render at 256 × 256 even if the map is 16384 elmos — this is a sanity preview, not the build. Houdini precedents this with HeightField Visualize: "Houdini supports a default display for height fields where height fields is a grey surface and the mask layer is colored red." (https://www.sidefx.com/docs/houdini/nodes/sop/heightfield_visualize.html).

4. **Apply button becomes "Commit to heightmap"** and is greyed while the expression is unparseable. The undo system already covers heightmap state, so Commit is undoable.

---

## 9. The first 30 seconds

The persona is a BAR community mapper with zero terrain-DCC experience whose target is a playable map in ~30 minutes. The "very very unintuitive" complaint (frustration #1) is almost certainly caused by an empty grey plane and no idea what to click.

**Recommended demo state on wizard close (or on "New project" with default template):**

- **Pre-populated 16 × 16 smu terrain**, seeded with a low-amplitude fractal noise (the existing Procgen "Hills" preset, amplitude ~80 elmos) so it does not look broken.
- **Two start positions** pre-placed at the canonical 25/75 diagonal: AllyTeam 0 at (4096, 4096), AllyTeam 1 at (12288, 12288). These satisfy the Spring `teams[0]`/`teams[1]` minimum from https://springrts.com/wiki/Mapdev:mapinfo.lua.
- **Symmetry preset Quad** so the user instantly sees axes in the viewport — explains §3 visually.
- **Camera framed** at a 35 ° downward angle, distance ~1.6 × map diagonal, so the whole map is in view.
- **First-launch hint overlay** (§6) with the three camera gestures.
- **A small dismissible "Next steps" `egui::Window` in the bottom-right** with three buttons: ① "Sculpt some hills" (selects Sculpt tool with Raise brush), ② "Place more start positions" (selects Start Positions tool), ③ "Build map" (presses Build).

This three-step affordance is borrowed directly from the Planetary Annihilation System Designer flow, where a Steam-published community walkthrough names every step verbatim: "Starting off we're gonna open the system designer, found in the main menu… By clicking 'New system' the game gives us a whole empty solar system to toy with… You can select a starting planet in 'Templates'… Now we click 'Edit planet' to make the moon just how we like it." (https://steamcommunity.com/sharedfiles/filedetails/?id=510312830). The Spring forum post-mortem on SpringMapEdit catalogues precisely the failure modes the pre-populated state plus framed camera plus reset-view nav gizmo collectively prevent: "Editing dialog does not resize itself when editing mode is changed… Camera mode is not restrained properly and allows it to go ludacris places, there is no way to reset the view from my knowledge." (https://springrts.com/phpbb/viewtopic.php?t=34159).

**On clicking Build with default state** the editor produces a valid (if dull) `.sd7` that loads in BAR. That is the proof-of-life moment. Nothing in the editor should be capable of producing a state where Build fails for the default project.

---

## 10. Reference inventory

| Editor | What it does well | Link to screenshot/doc |
|---|---|---|
| Blender Sculpt mode | Vertical icon toolbar on the left edge listing every sculpt tool, mode-categorised; symmetry as X/Y/Z toggle buttons with per-axis radial fold count | https://docs.blender.org/manual/en/latest/sculpt_paint/sculpting/toolbar.html ; https://docs.blender.org/manual/en/latest/sculpt_paint/sculpting/tool_settings/symmetry.html |
| Blender Viewport Gizmos | Coloured-axis navigation gizmo with click-to-snap orthographic views and drag-to-orbit, top-right of 3D viewport | https://docs.blender.org/manual/en/latest/editors/3dview/display/gizmo.html |
| Blender A.N.T. Landscape | Preset-first procgen with live "Adjust Last Operation" panel (Main / Noise / Displace settings) | https://docs.blender.org/manual/en/4.1/addons/add_mesh/ant_landscape.html |
| World Machine 4 | Leftside View vertical panel with preview window, tracing overlay, contour lines, device list — proves the left-column pattern for terrain tools | https://help.world-machine.com/topic/manual-4-side-panel/ ; https://help.world-machine.com/topic/terrain-views/ |
| Houdini Heightfield SOPs | Layered height + mask model rendered as red tint on the 3D surface; Draw Mask invoked via in-viewport Handles tool | https://www.sidefx.com/docs/houdini20.0/model/heightfields.html ; https://www.sidefx.com/docs/houdini/nodes/sop/heightfield_drawmask.html |
| Unity Terrain Editor | Five-icon Terrain toolbar with Paint Terrain dropdown (Raise/Lower/Smooth/Stamp); on-canvas cylindrical brush cursor; `[` `]` to resize | https://docs.unity3d.com/6000.1/Documentation/Manual/terrain-UsingTerrains.html ; https://docs.unity3d.com/6000.0/Documentation/Manual/terrain-Tools.html |
| Planetary Annihilation System Designer | Main menu → New system → Edit planet template → slide-bar parameters (radius / height / water / biome / seed) — the named target UX feel | https://steamcommunity.com/sharedfiles/filedetails/?id=510312830 ; https://planetaryannihilation.com/guides/controls-and-the-user-interface/ |
| WorldEdit (Bukkit/Forge) | `//pos1` `//pos2` selection staging, `//drawsel` outline visualisation, `//gen` expression parser, `//undo` history | https://worldedit.enginehub.org/en/latest/usage/regions/selections/ ; https://github.com/EngineHub/WorldEditDocs/blob/master/source/usage/generation.rst |
| Krita | Tool options docker visible by default in default workspace layout; Toolbox docker on left edge of canvas | https://docs.krita.org/en/reference_manual/resource_management/resource_workspace.html ; https://bugs.kde.org/show_bug.cgi?id=348730 |
| Aseprite | Symmetry mode with draggable on-canvas handles for each axis — the 2D mirror-axis convention | https://www.aseprite.org/docs/symmetry/ |
| SpringMapEdit | Single-window UI with brushmode-aware controls; documents what to avoid (camera reset missing, editing dialog does not resize, smoothing too weak) | https://github.com/aeonios/SpringMapEdit ; https://springrts.com/phpbb/viewtopic.php?t=34159 |
| tebeer/BARMapEdit | Direct competitor: Unity + Dear ImGui-based editor (52.4 % C#, 37.2 % ShaderLab, 5.2 % HLSL, 5.2 % Lua) with mapinfo.lua / TDF parser panes, Properties / Console / Textures windows. Diverge by being native Rust and brush-driven rather than form-driven (no README, 22 commits, 0 stars, 1 watcher as of May 2026) | https://github.com/tebeer/BARMapEdit |
| BAR Spring mapinfo.lua | Canonical `teams = { [0] = { startPos = { x, z } }, ... }` schema, 0-indexed; BAR FFA gadget documents the flat-list limitation that Section 4 solves | https://springrts.com/wiki/Mapdev:mapinfo.lua ; https://github.com/beyond-all-reason/Beyond-All-Reason/blob/master/luarules/configs/ffa_startpoints/README.md ; https://www.beyondallreason.info/guide/map-checklist |
| egui panel model | `TopBottomPanel`, `SidePanel`, `CentralPanel` add-order rule; CentralPanel last; `Painter` overlays inside CentralPanel | https://docs.rs/egui/latest/egui/containers/panel/ |

---

## Recommendations (staged, decision-ready)

**Phase 3.1 — ship in the first sprint (resolves #1, #3, #5, #7, #8):**
1. Split the window into the five-zone shell from §1. Move Tool to the left strip, parameters to the right Inspector, Build to the top bar, camera readout + validation to the bottom strip.
2. Add the brush-preview ring on the canvas (§5). Single afternoon's work; biggest perceived-quality jump per LoC.
3. Add the nav gizmo (§6 affordance 1) and the first-launch hint overlay (§6 affordance 2).
4. Promote symmetry to the top-bar widget plus dashed-axis viewport overlay (§3).

**Phase 3.2 — second sprint (resolves #2, #4, #6):**
5. Rebuild the Start Positions editor around allyteams (§4). Drag-paint, multi-select, cross-tool visibility rule.
6. Procgen live-parse + preset-first reorder + preview thumbnail (§8).
7. `?`-key cheat-sheet modal (§6 affordance 3).

**Phase 3.3 — polish:**
8. Pre-populated demo state on first launch (§9).
9. Status-strip clickable chips that jump-to-tool.

**Benchmarks that would change these recommendations:**
- If the egui frame budget cannot sustain a 256 × 256 procgen preview at a 50 ms debounce on a target laptop GPU (Intel UHD 620 baseline), drop the live preview and ship only the live-parse underline. The preview becomes Phase 4.
- If telemetry from the tracing layer after Phase 3.1 ships shows ≥30 % of users never touch the left tool strip, add a one-time pulsing-glow tour the second time the app launches.
- If user-testing on the §9 demo state still produces "unintuitive" feedback, add an in-viewport floating chip *"Try dragging with right mouse button to look around"* that disappears after the first orbit.

---

## Caveats

- **No screenshots of `tebeer/BARMapEdit` UI are public** — the repo has no README, 22 commits, 0 stars, and no releases. Our reconstruction of its layout is inferred from window names in its committed `imgui.ini` (Editor, mapinfo.lua, maphelper/parse_tdf.lua, Lighting, Properties, Console, Textures) — not from a documented design intent. Treat that row of the reference table as competitive intelligence, not as a pattern to copy.
- **`Jandodev/bar-editor` could not be located.** Jandodev's GitHub profile shows nine repositories, none matching `bar-editor`, `map`, `editor`, or `terrain`. Either the repo is private, was renamed, or the brief's reference is stale. Do not block Phase 3 on integrating it.
- **The Spring `mapinfo.lua` `teams` table has no documented hard cap** on the number of start positions, but the engine warns rather than fails when start-script teamIDs lack a `startPos`. The editor must therefore decide its own UI ceiling; 32 positions per allyteam × 8 allyteams is a safe upper bound that comfortably exceeds any current BAR mode.
- **The Blender Studio Fundamentals training pages** (studio.blender.org) are intermittently 403 to unauthenticated fetches; all citations in this document use the equivalent stable docs.blender.org manual URLs where possible.
- **Single-window, egui-only constraint** has been honoured: every layout element above resolves to documented `TopBottomPanel` / `SidePanel` / `CentralPanel` / `Window` / `Painter` primitives. No third-party dock crate, no flexbox, no retained tree. If egui ever ships first-party docking, this document does not need to change — it just becomes a safe default.