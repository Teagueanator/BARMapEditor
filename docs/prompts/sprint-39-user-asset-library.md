# Sprint 39 — F23 User-asset library: stamps + feature prefabs + DNTS material packs

Paste the block below into a fresh Claude / Claude Code session.

---

You are continuing work on the BAR Map Editor, a Rust + egui + wgpu desktop
GUI for authoring Beyond All Reason / Recoil maps. Branch is `main`.

This is **Sprint 39** — implements **F23** (user-asset library) from
the SRS QoL additions. The user can:

- Save a hand-painted area of the map as a **stamp**, then re-paste
  it elsewhere (PA-style brush stamps).
- Save a feature placement pattern as a **feature prefab** (e.g., "5
  pine trees in a circle around a rock").
- Bundle a set of DNTS slot assignments + tex_scale / tex_mult
  config as a **DNTS material pack** for re-use across projects.

The library lives in `~/.local/share/barme/library/` (XDG data dir),
indexed by user-set tags.

**Prerequisites:**
- Sprint 37 (brushes + symmetry line) — Stage 1 + renderer-parity
  complete; the editor is stable.
- Sprint 22 (help center) — library articles live there.

## Step 1 — Read the context

1. `/home/teague/code/BARMapEditor/CLAUDE.md` — house rules.
2. `/home/teague/code/BARMapEditor/SRS.md` — F23 user-asset library
   STATUS UPDATE note (architectural sketch in §3 / status updates).
3. `/home/teague/code/BARMapEditor/crates/barme-core/src/project.rs`
   — `Project` is the source for what a stamp captures.

## Step 2 — Devlog flow

```bash
./devlog/log.sh new sprint-39-user-asset-library
```

## Step 3 — Scope

In order, one commit per chunk:

### 1. Library data model + registry

`crates/barme-core/src/library.rs` (new):

```rust
pub struct LibraryEntry {
    pub uuid: Uuid,
    pub kind: LibraryEntryKind,
    pub name: String,
    pub tags: Vec<String>,
    pub created_at: SystemTime,
    pub thumbnail_path: PathBuf,
}

pub enum LibraryEntryKind {
    HeightmapStamp { width: u32, height: u32, data_path: PathBuf },
    FeaturePrefab { feature_list_path: PathBuf },
    DntsMaterialPack { config_path: PathBuf },
}

pub struct LibraryRegistry {
    pub root: PathBuf,  // ~/.local/share/barme/library/
    pub entries: Vec<LibraryEntry>,
}

impl LibraryRegistry {
    pub fn load(root: &Path) -> Result<Self, Error>;
    pub fn add(&mut self, kind: LibraryEntryKind, name: String, tags: Vec<String>);
    pub fn remove(&mut self, uuid: Uuid);
    pub fn search(&self, query: &str) -> Vec<&LibraryEntry>;
    pub fn apply(&self, uuid: Uuid, project: &mut Project, target_pos: Vec3);
}
```

Per CLAUDE.md / SRS architectural note: the library belongs in
`barme-core` as a registry layer. `barme-pipeline` does NOT
bundle library assets into the `.sd7` — at build time, library
references resolve and the raw data is staged like any other
project asset.

### 2. UI — library panel

`crates/barme-app/src/ui/library_panel.rs` (new). Top-bar menu
item `View > Library` opens an `egui::Window`.

Layout: tag-filtered grid of entries (thumbnail + name + delete).
Each entry has a primary action ("Apply"). Heightmap stamps need
a position; clicking Apply enters "place mode" (cursor follows
mouse; LMB places). Feature prefabs work similarly. DNTS packs
apply immediately to the current project.

### 3. Saving entries

"Save to library" affordances:
- **Heightmap stamp**: in Sculpt mode, a "Lasso save" tool —
  drag a rectangle on the canvas; right-click → "Save to library".
- **Feature prefab**: in Feature mode, multi-select features
  (Ctrl-click), right-click → "Save selection".
- **DNTS material pack**: in PaintLayer mode's layers panel,
  a "Save material" button.

Each save prompts for name + tags via a small modal (Sprint 31's
primitive).

### 4. Apply / paste workflow

For heightmap stamps: pasted via the existing brush apply
infrastructure (radius-less; cursor center = stamp center).
Snap to map bounds.

For feature prefabs: each feature placed at offset from cursor,
heading-rotated by user-specified angle.

For DNTS material packs: apply replaces the current
`LayerStack`'s slot/scale/mult configuration. Confirmation modal:
"Replace current layer settings? (Ctrl+Z to undo)".

### 5. Library serialisation

Each entry stored as:
- `<root>/<uuid>/manifest.toml` — kind + name + tags + created_at.
- `<root>/<uuid>/thumbnail.png` — 128² preview.
- `<root>/<uuid>/data.{png,toml}` — kind-specific payload.

Heightmap stamps: PNG (R16 raw or sRGB normalised — both work).
Feature prefabs: TOML with feature list.
DNTS packs: TOML with slot config.

### 6. Tests + rollup

- **Round-trip**: save → reload → apply produces identical result.
- **Library scan**: empty dir + populated dir + corrupted entry.
- **UI smoke**: library panel renders + filters + apply works.
- **Rollup**: STATUS UPDATEs (F23 done). "Sprint 40 = F13 .sd7
  import" handoff.

## Step 4 — Standing constraints

Same as prior sprints. Tracing: `info!` on library load + entry
counts; `warn!` on corrupted entries.

## Step 5 — Out of scope

- **Cloud sync** — strictly local-only.
- **Sharing via URL** — Stage 3 stretch.
- **Procedural template library** (Quicksilver/Glitters/etc.) —
  separate sprint.
- **F19 procedural feature scatter** — Stage 3.

## Step 6 — Critical pitfalls

1. **`~/.local/share` on Windows / macOS**: use `directories`
   crate for the platform-correct path. Don't hardcode.

2. **Library size cap**: no enforcement; the user manages. But
   surface a "Library size: N entries, M MB" status in the
   library panel.

3. **UUID collisions in import** (Sprint 40 may import library
   entries embedded in projects): use a UUID v4 generator;
   collision-free.

4. **Apply must go through ProjectDiff** for undo.

5. **Thumbnail generation**: heightmap stamp = grayscale render.
   Feature prefab = top-down render of placed features. DNTS pack
   = composite preview at thumbnail-res.

## Step 7 — Exit criteria

- 5+ commits on `main`: data model, UI, save flows, apply flows,
  rollup.
- 1 devlog folder filled.
- SRS / ROADMAP STATUS UPDATEs (F23 done).
- `cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` green.
- Smoke test: save 3 stamps + 1 prefab + 1 DNTS pack. Apply each
  to a fresh project. Library persists across editor restarts.
- Final devlog: summary + "Sprint 40 = F13 .sd7 import" handoff.
