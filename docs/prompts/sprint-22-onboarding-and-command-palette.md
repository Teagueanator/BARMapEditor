# Sprint 22 — Onboarding + contextual help + command palette (U2)

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 22** — the third and final UI/UX polish sprint
(19 / 20 / 22). It closes the **onboarding** loop. The 2026-05-20
UX audit found:

- The `?` chord (Shift+/) is the **only** help discovery channel,
  and the chord itself is undocumented.
- The intro Window dismisses once per editor version and never
  reopens — users who dismiss accidentally are stranded.
- The Layers panel + paint viewport (Sprints 15-17, ~1500 LoC of new
  surface area) has no per-tool intro. A user pressing `L` from
  Sculpt has no idea what they're looking at.
- There's no command palette. A new user looking for "Build" must
  scan the top bar; a user looking for "set extractor radius for
  all spots" has to know it's in the metal inspector.
- Validation chip / build-failure errors have no path to the
  underlying PITFALLS.md content.

After this sprint:

- A persistent **Help icon** in the top-bar opens a re-openable
  **help center** Window with articles per tool, per pitfall, and
  a "What's new" pane.
- A first-launch **guided tour** highlights each panel in sequence
  with callouts (uses Sprint 19's `help_text` catalogue).
- Per-tool **intro overlays** the first time a user enters each
  tool — especially PaintLayer (the biggest new surface).
- A **Ctrl-K command palette** lists every tool, menu item,
  preset, and keyboard shortcut by name.
- A **"What's this?" hover-popover mode** (Ctrl+Shift+H toggles)
  turns Sprint 19's tooltips into pinned popovers for prolonged
  exploration.
- Help articles linked from lint-panel rule names + build-overlay
  errors.

**Prerequisites:**
- Sprint 19 (UI tooltip + help-text pass) MUST be ticked. The
  help_text catalogue is the source of strings the tour reuses.
- Sprint 20 (async build + log) MUST be ticked. Build-overlay error
  states link into the help center.
- Sprint 21 (lint pass) MUST be ticked. Lint-rule rows in the panel
  link to per-pitfall articles.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — §1.4 user stories
   (each story is a candidate tour path), §3.3 NFR-Discoverability.
3. `/home/teague/code/BARMapEditor/docs/PITFALLS.md` — every
   numbered pitfall becomes a help article.
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/help_text.rs`
   (Sprint 19) — the catalogue. The tour and intro overlays
   reuse these strings; the help center expands them into
   articles.
5. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/cheat_sheet.rs`
   — existing keyboard-shortcut surface. The help center
   subsumes this into its "Shortcuts" tab; cheat_sheet stays
   as the `?` chord backstop.
6. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/intro.rs`
   — the version-keyed first-launch hint. Replace with a
   "first-launch tour" entry point that links to the new help
   center.
7. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/next_steps.rs`
   — per-project post-wizard hint. Keep; extend to include a
   "Start the tour" button.
8. `/home/teague/code/BARMapEditor/crates/barme-app/src/config.rs`
   — `EditorConfig`. Add `tour_completed_for: Option<String>` (version
   string) and `tool_intros_seen: HashSet<Tool>`.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new stage-1-onboarding-and-help-center
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. Help center module + content articles

**New module:** `crates/barme-app/src/ui/help_center.rs`.

```rust
pub struct HelpCenter {
    pub open: bool,
    pub active_article: Option<HelpArticleId>,
    pub search: String,
}

pub enum HelpArticleId {
    GettingStarted,
    WhatsNew,
    Tool(Tool),                 // 9 articles — one per Tool variant
    Pitfall(u8),                // §1 through §28
    Shortcuts,
    BuildPipeline,
    LayeredPainter,
    AllyTeams,
    WaterAndLava,
}

pub fn help_window(ctx: &egui::Context, hc: &mut HelpCenter);
```

Articles are **inline `&'static str`** for Sprint 22. Don't read
from disk at runtime — bake the markdown into the binary. The
content lives in `crates/barme-app/src/ui/help_content/` with one
file per article (e.g. `tool_paint_layer.md`,
`pitfall_04_heightmap_dims.md`). Use `include_str!` to inline.

Layout (`help_window`):
```
┌──────────────────────────────────────────────────┐
│ Help                          [Search: ____] [×] │
├────────────┬─────────────────────────────────────┤
│ Articles   │ # Layered painter                   │
│ ──────────│                                     │
│ Getting    │ The Layered Painter (Sprints 15-17)│
│ started   │ replaces the legacy 4-channel splat │
│ What's new │ inspector with a Photoshop-style    │
│ Tools     ▶│ stack…                              │
│ Pitfalls  ▶│ ...                                 │
│ Shortcuts │ ## Adding a layer                    │
│ ...        │ 1. Press L to enter PaintLayer mode.│
│           │ 2. ...                              │
└────────────┴─────────────────────────────────────┘
```

Render markdown via `egui_commonmark` (add to workspace deps; ~50
KB compiled). Articles span 200-600 lines each; bake conservatively.

Search filter (textual): substring match on article body.

### 2. Guided tour mode

**New module:** `crates/barme-app/src/ui/tour.rs`.

```rust
pub struct TourState {
    pub active: bool,
    pub step_index: usize,
    pub steps: Vec<TourStep>,
}

pub struct TourStep {
    pub target_rect: egui::Rect,  // the panel/widget to highlight
    pub callout_text: &'static str,
    pub action_text: Option<&'static str>,  // e.g. "Press B to enter Sculpt mode"
}
```

The tour:
1. Project header — "this is your project metadata".
2. Tool strip — "9 tools; B = Sculpt, L = PaintLayer, etc."
3. Inspector — "tool-specific options".
4. Canvas — "left-drag to apply; right-drag to orbit".
5. Minimap — "auto-updates as you paint".
6. Status strip — "build state + validation".
7. Help icon — "click any time".

Each step:
- Renders a darkened backdrop with a cutout around `target_rect`.
- Pops a callout `egui::Window` with the text + `[Next]`/`[Skip
  tour]` buttons.
- Advances on `[Next]` or after 8 s of inactivity.

**Wizard integration**: at the end of the wizard, the
`next_steps.rs` Window adds a `[Start the tour]` button. Triggers
the tour automatically on first project ever.

**Re-trigger**: a `Help > Start guided tour` menu item under the
help center. Resets `tour_completed_for` and runs from step 1.

### 3. Per-tool intro overlays

**New module:** `crates/barme-app/src/ui/tool_intro.rs`.

When the user enters a tool for the first time
(`tool_intros_seen` doesn't contain it), pop a non-modal
`egui::Window` in the central viewport:

```
┌────────────────────────────────────┐
│ Paint Layer                      × │
│                                    │
│ Switch to the top-down 2D viewport │
│ to paint into per-layer masks.     │
│                                    │
│ Left-drag: paint with active brush │
│ Right-drag: orbit camera           │
│ Middle-drag: pan viewport          │
│ Scroll: zoom                       │
│                                    │
│ [Open Layers panel]                │
│ [Read more in Help Center]         │
│ [Don't show this again]            │
└────────────────────────────────────┘
```

The "Don't show this again" pin persists to `EditorConfig`. A
`Help > Reset tool intros` menu item clears the set.

Per-tool content:
- **Select** — orbit camera basics.
- **Sculpt** — brushes, radius/strength, symmetry, undo.
- **StartPositions** — drag-paint, ally teams, presets.
- **MetalSpots** — extractor radius, BAR yield conventions.
- **GeoFeatures** — what geo vents do in BAR economy.
- **Feature** — category filter, rotation, stock features.
- **Water** — preset chips, MVP rendering, polish coming
  Sprint 26.
- **PaintLayer** — the painter overview (subsumes Layers panel
  intro).
- **Procgen** — preset → custom expression, preview thumbnail,
  apply-to-heightmap is undoable.

### 4. Ctrl-K command palette

**New module:** `crates/barme-app/src/ui/command_palette.rs`.

```rust
pub struct CommandPalette {
    pub open: bool,
    pub query: String,
    pub commands: Vec<Command>,  // populated once at app startup
    pub selected_index: usize,
}

pub struct Command {
    pub label: &'static str,        // "Open project"
    pub shortcut: Option<&'static str>,  // "Ctrl+O"
    pub category: CommandCategory,
    pub action: CommandAction,
}

pub enum CommandCategory {
    File, Edit, View, Build, Tools, Help, Presets,
}

pub enum CommandAction {
    OpenWizard,
    SaveProject,
    SaveAs,
    OpenProject,
    QuitApp,
    Undo,
    Redo,
    SwitchTool(Tool),
    OpenHelp(HelpArticleId),
    StartTour,
    ToggleWireframe,
    ToggleLighting,
    ApplyWaterPreset(WaterPreset),
    ApplyProcgenPreset(ProcgenPreset),
    BuildAndInstall,
    OpenLintPanel,
    OpenBuildLog,
    OpenHelpCenter,
    // ...
}
```

Triggered by **Ctrl+K**. Opens a centred `egui::Window` with a
text input + scrollable result list. Arrow keys navigate; Enter
executes; Esc closes. Match is fuzzy (substring on label +
category).

Population: build the command list at app startup from a
deterministic registration table. Every menu item, every tool,
every preset gets a `Command`.

### 5. "What's this?" hover-popover mode

**Toggle**: `Ctrl+Shift+H`. Stores in `App::whats_this_mode: bool`.

When active:
- The cursor gets a `?` overlay.
- Hovering any widget displays Sprint 19's hover-text as a
  **pinned popover** (right of the cursor) that persists until
  the user moves the cursor elsewhere or clicks.
- The pinned popover has a "Read more" link to the relevant
  `HelpArticleId`.

Implementation: extend egui's `Response::on_hover_text` mechanism
via a thin wrapper `help_text::show_popover(ui, response, id)`
that consults `whats_this_mode` and pins the popover if set.

### 6. Help-center entry points

- **Top-bar Help icon** (Sprint 19 wired this to the cheat sheet).
  Re-wire to open the help center; the cheat sheet becomes a tab
  inside the help center.
- **Validation chip click** (Sprint 19 → lint panel). Each lint
  rule row now has a `[Help…]` button → opens the corresponding
  PITFALLS article in the help center.
- **Build-overlay failure state** (Sprint 20): the log panel
  gains a `[What does this mean?]` button when stderr contains a
  known PyMapConv error → opens the BuildPipeline article.
- **Wizard next-steps** Window: `[Start the tour]` button.
- **Layers panel empty state** (Sprint 17 / `layers_panel.rs:182`):
  the "Empty stack — click 'Add layer' to start" text gets a
  `[How layers work]` link → LayeredPainter article.
- **Tool intro overlays**: each has a `[Read more in Help Center]`
  button.

### 7. Tests + smoke run + rollup

- **Article-content test**: every `HelpArticleId` variant has
  non-empty content (compile-time check via macro).
- **Command registration test**: assert ≥40 commands registered;
  every tool has a `SwitchTool(Tool)` command.
- **Tour-completion persistence test**: complete tour → restart
  editor → tour does not auto-trigger.
- **Manual smoke** (record in devlog):
  - Fresh editor (no `EditorConfig`) → wizard → click "Start
    the tour" → 7-step tour walks panels in order.
  - First entry into PaintLayer → tool intro pops; dismiss
    "Don't show again" → re-entry shows nothing; reset intros
    from menu → intro pops again.
  - Ctrl+K → palette opens; type "build" → top result is
    "Build and install (Ctrl+B)" → Enter → build starts.
  - Ctrl+Shift+H → cursor gets `?` overlay; hover
    `extractor_radius` field → popover pins with hover-text +
    "Read more" link → click → help center opens to the
    relevant pitfall article.
  - Lint panel rule row → `[Help…]` → opens corresponding
    article.

- **Rollup commit**: STATUS UPDATEs in SRS / ROADMAP (U2 shipped;
  three UI sprints complete). Closing devlog log. "Sprint 23 =
  Sprint-17 cleanup (16-SMU OOM + orphan-texture GC + legacy
  SplatConfig retire)" handoff note.

## Step 4 — Standing constraints

Same as prior sprints. The `egui_commonmark` dep adds ~50 KB to
the binary. Tracing: `trace!` on help-center open/close, tour step
transitions, command-palette executes.

## Step 5 — Out of scope

- **Help article content beyond a baseline 1-2 paragraphs each**.
  Articles can grow over time; Sprint 22 ships the framework + the
  initial set. Future polish (Sprint 28+) extends articles.
- **Animated tour callouts** (GIF examples) — text only.
- **i18n / localisation** — strings are English-only; the
  `help_text` catalogue is the source of truth; localisation is
  Stage 3.
- **Interactive tutorials** (sandbox project that walks the user
  through painting a 4-SMU map) — Stage 2 polish.
- **Toast queue / proper confirmation modals** — Sprint 31. The
  command palette executes immediately; destructive commands
  ("Discard project") are excluded for now.
- **Sprint 23 cleanup work** (orphan-texture GC, OOM
  root-cause) — not this sprint.

## Step 6 — Critical pitfalls (read twice)

1. **The tour can interfere with active work.** Only auto-trigger
   on a brand-new project (no prior save). Provide a one-click
   `[Skip tour]` option that persists for the editor version.

2. **Per-tool intro overlays are NON-blocking** — they don't grab
   focus. The user can dismiss by clicking outside, pressing Esc,
   or interacting with the tool. The "Don't show again" only
   persists when the user explicitly checks the box, not on Esc-
   dismiss — Esc means "I get it for now; show me next time".

3. **Command palette must NOT execute on type.** Selection +
   Enter is the action gesture. Arrow keys navigate. Esc closes.

4. **Help center articles use `include_str!`** — content lives in
   the source tree under `crates/barme-app/src/ui/help_content/`.
   This means editing an article requires a recompile. That's
   acceptable for Sprint 22; Stage 2 polish may move to runtime
   loading.

5. **`egui_commonmark` styling**: defaults are OK but the
   editor's dark theme needs the markdown renderer's colours
   pinned. Audit headings / links / code blocks against
   `Tokens::DARK`.

6. **Tour rect calculation**: panels move when the user resizes
   the inspector. Snapshot the rect at tour-step transition
   (don't reuse a stale rect from the previous step). If a
   targeted widget doesn't exist (e.g., the user already closed
   a panel), skip the step.

7. **Command palette population**: build the command list at app
   startup, not per-frame. Mutate when state changes (e.g., new
   recent project → push a `OpenProject(path)` command).

8. **"What's this?" mode persistence**: do NOT persist
   `whats_this_mode` across editor restarts. It's an exploration
   tool, not a default — restarts reset to off.

9. **Help center tab strip**: tabs should be alphabetical within
   each category, but Getting Started + What's New always
   pinned to the top. Don't auto-collapse the "Tools" / "Pitfalls"
   trees on every open.

10. **Don't break `?` chord**: the cheat sheet stays available
    via `?` even when the help center is open. It's the keyboard-
    shortcut backstop; users who want shortcuts only shouldn't
    need to navigate the full help center.

11. **Tour autoplay vs accessibility**: 8s inactivity advance is
    fine for sighted users; consider providing an explicit
    "manual mode" toggle that disables autoplay. Defer the
    toggle to Stage 2 polish but DO add `prefers-reduced-motion`
    detection if egui exposes it.

## Step 7 — Exit criteria

- 7+ commits on `main`: help center + content, tour, tool intros,
  command palette, what's-this mode, entry-point wiring, rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (U2 shipped; three UI sprints done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: see Step 3 chunk 7.
- Help articles: ≥9 tool articles + ≥15 pitfall articles + 4
  meta articles (Getting Started, What's New, Shortcuts,
  BuildPipeline) = ≥28 articles total. Each ≥3 paragraphs.
- Command palette: ≥40 commands.
- Tour: 7+ steps; completes in <90 s of user time.
- Final devlog log: summary + "Sprint 23 = Sprint-17 cleanup
  (16-SMU OOM, orphan-texture GC, legacy SplatConfig retire)"
  handoff note.

Start by scaffolding the `help_content/` directory and writing
the 9 tool articles + 5 priority pitfall articles (§4 heightmap
dims, §6 mapinfo deps, §11 sundir, §13 metalmap, §15
splatDetailNormalTex). The framework is mechanical once content
exists.
