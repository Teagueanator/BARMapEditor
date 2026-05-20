# Sprint 45 — F21 Light/dark theme toggle + F22 live CPU/RAM status

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 45** — two small QoL items from the SRS Stage-2
list:

1. **F21** — Light/dark theme toggle, persisted across editor
   sessions. Today the editor ships only `Tokens::DARK` (Sprint 9
   / ADR-035).
2. **F22** — Bottom status bar with live CPU% + resident memory.
   Today the status strip shows camera + map dims + lint count
   only.

After this sprint, the editor is a more comfortable tool for both
late-night and bright-room editing, and users can see resource
usage at a glance.

**Prerequisites:**
- Sprint 27 (inspector consistency refactor) — the theme
  switching touches all section/chip widgets, which have been
  pattern-unified.
- Sprint 31 (toast queue) — theme switch is a toast confirmation.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F21 + F22.
3. `/home/teague/code/BARMapEditor/crates/barme-app/src/ui/theme.rs`
   — `Tokens::DARK` palette. Sprint 45 adds `Tokens::LIGHT`.
4. `/home/teague/code/BARMapEditor/crates/barme-app/src/config.rs`
   — `EditorConfig`. Add `theme: ThemeKind`.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-45-theme-and-status-bar
```

## Step 3 — Scope

### 1. Light theme palette

`crates/barme-app/src/ui/theme.rs` extension:

```rust
pub enum ThemeKind { Dark, Light }

pub struct Tokens {
    pub bg, panel_bg, text, muted, accent, accent_dim,
    pub success, danger, warn, info,
    pub canvas_bg, grid_line, symmetry_axis,
}

impl Tokens {
    pub const DARK: Tokens = { ... };
    pub const LIGHT: Tokens = Tokens {
        bg: Color32::from_rgb(248, 249, 250),
        panel_bg: Color32::from_rgb(255, 255, 255),
        text: Color32::from_rgb(33, 37, 41),
        muted: Color32::from_rgb(108, 117, 125),
        accent: Color32::from_rgb(0, 123, 255),
        accent_dim: Color32::from_rgb(108, 147, 204),
        success: Color32::from_rgb(40, 167, 69),
        danger: Color32::from_rgb(220, 53, 69),
        warn: Color32::from_rgb(255, 193, 7),
        info: Color32::from_rgb(23, 162, 184),
        canvas_bg: Color32::from_rgb(240, 242, 245),
        grid_line: Color32::from_rgb(200, 200, 200),
        symmetry_axis: Color32::from_rgb(150, 100, 50),
    };
}

pub fn current_tokens(kind: ThemeKind) -> &'static Tokens {
    match kind { ThemeKind::Dark => &Tokens::DARK, ThemeKind::Light => &Tokens::LIGHT }
}
```

Audit every reference to `Tokens::DARK` and switch to
`current_tokens(self.theme)`. Most references are in `widgets.rs`
+ `overlay.rs`.

### 2. Theme switch UI

`View > Theme` submenu: Dark / Light radio buttons. Click
applies + persists to `EditorConfig`. Toast info: "Theme: Light"
(or Dark).

Also expose in Sprint 22's command palette (Ctrl+K → "Theme"
results).

### 3. Live CPU + RAM status bar

Status strip (`main.rs:6011`) gains two new chips:
- **CPU**: 1Hz updated; uses `sysinfo` crate's
  `System::cpu_usage_self()`. Format: "CPU: 12%".
- **RAM**: 1Hz updated; uses `sysinfo::Process::memory()` for
  RSS. Format: "RAM: 487 MB".

Both chips have tooltips with deeper info ("System has 16 GB; the
editor's peak this session was 612 MB").

Add `sysinfo = "0.30"` to workspace deps.

### 4. Tests + rollup

- **Theme palette tests**: hex codes are distinct between dark
  + light; contrast ratios pass WCAG AA for body text.
- **CPU / RAM smoke**: launch editor → wait 2s → status shows
  values > 0.
- **Persistence**: change theme → restart editor → theme
  preserved.
- **Rollup**: STATUS UPDATEs (F21 + F22 done; Stage 2 F-list
  effectively complete).

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on theme switch with new
kind; `trace!` on CPU/RAM samples (high volume; gated).

## Step 5 — Out of scope

- **Custom theme editor** (let the user pick colours) — Stage 3.
- **Per-tool theme overrides** — no.
- **HDR theme** — out of scope.
- **GPU utilisation** in status bar — Stage 2 polish.

## Step 6 — Critical pitfalls

1. **Theme switch live-applies**: don't require a restart. egui
   redraws the next frame.

2. **`Tokens::DARK` constant references**: there are many. Audit
   carefully — missing one results in a stuck-dark widget on
   light theme.

3. **Contrast accessibility**: WCAG AA requires 4.5:1 body
   contrast. Check via an online checker. Don't ship colours
   that fail.

4. **Symmetry axis colour on light theme**: dark theme uses cyan;
   light theme switches to brown for visibility against the
   light canvas. Tested visually.

5. **`sysinfo` on Windows**: requires `Threads` feature for
   per-process CPU. Add to feature flags.

6. **CPU sampling overhead**: 1Hz is cheap (~10µs per sample).
   Don't bump to higher freq.

7. **RAM is RSS, not heap**: includes shared libs. On Linux that
   means GTK/Vulkan/Mesa ~150 MB minimum. Document.

## Step 7 — Exit criteria

- 3+ commits on `main`: light palette + audit, status bar chips,
  rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (F21 + F22 done; Stage 2 F-list
  effectively complete).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: View > Theme > Light → editor adopts light colours.
  Status strip shows live CPU/RAM. Restart → theme preserved.
- Final devlog: summary + Stage 2 F-list-complete announcement.
  Beyond this: Stage 3 (F18 GeoTIFF, F19 procedural feature
  scatter, F20 publish-to-BAR) + L2 lint refinements + procedural
  template library + any user-requested feature work.
