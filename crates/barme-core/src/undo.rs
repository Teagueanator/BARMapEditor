//! Unified undo / redo over heightmap strokes AND project-level edits.
//!
//! ADR-033 introduced per-stroke copy-on-first-write for heightmap edits.
//! B5 keeps that machinery verbatim and grows the data model around it:
//! the history stack now holds an enum of either a heightmap stroke or a
//! project-level diff (start-position place/move/delete, wizard apply).
//! One stack, one Ctrl-Z hits whichever entry sits on top.
//!
//! ## Eviction policy — largest-first, not strictly oldest
//!
//! Heightmap entries are bytes-heavy (a 16-SMU stroke at radius 1024 can
//! commit ~2 MB). Project diffs are kilobytes at most (Place/Delete are
//! tens of bytes; ApplyWizard with a few hundred start positions tops out
//! in the low kilobytes). Under the previous "evict the oldest" rule a
//! single long stroke would happily evict the last 20 F8 placements just
//! to make room. Sharing a 100 MB cap is still fine — the budget
//! comfortably holds ~50 long strokes — but eviction has to prefer the
//! *largest* committed entry, otherwise the cap lopsidedly punishes the
//! lighter channel.
//!
//! ## Brush-stroke capture path (unchanged from ADR-033)
//!
//! The in-flight stroke state — heightmap-sized scratch buffer + packed
//! bitset — lives inside [`History`] and is committed via
//! [`History::end_stroke`] into a [`HeightmapEntry`]. See the original
//! ADR-033 prose for the rationale; B5 doesn't touch any of it.
//!
//! ## Project-diff capture path (B5)
//!
//! Callers push project diffs directly with
//! [`History::push_project_diff`]. The data shape is symmetric: each
//! variant carries enough state to apply itself in either direction, and
//! `apply` on undo means "revert this", `apply` on redo means "re-do
//! this." The app dispatches the variant via [`History::pop_undo`] /
//! [`History::pop_redo`] + [`History::push_to_redo`] /
//! [`History::push_to_undo`] — the project mutation lives in the app
//! because the field set crosses the barme-core / barme-app boundary.

use std::collections::VecDeque;

use tracing::{trace, warn};

use crate::MapSize;
use crate::brushes::DirtyRect;
use crate::heightmap::Heightmap;
use crate::procgen::Domain;
use crate::project::{AllyGroup, GeoVent, MetalSpot, StartPosition};
use crate::symmetry::SymmetryAxis;

/// One committed undo entry. Either a heightmap stroke (the ADR-033
/// shape) or a project-level diff.
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryEntry {
    Heightmap(HeightmapEntry),
    Project(ProjectDiff),
}

impl HistoryEntry {
    /// Approximate bytes this entry occupies, used by the cap eviction.
    /// Counts heap allocations explicitly; small inline state (enum
    /// tags, fixed-size fields) is approximated by `size_of_val`.
    pub fn bytes(&self) -> usize {
        match self {
            HistoryEntry::Heightmap(h) => h.bytes(),
            HistoryEntry::Project(p) => p.bytes(),
        }
    }
}

/// One heightmap stroke. The bbox is the union of every pixel the stroke
/// touched (computed at `end_stroke` from the per-stroke bitset);
/// `before` holds the bbox-shaped pre-stroke pixels in row-major order.
/// After [`Heightmap::swap_rect`] the same buffer holds the *current*
/// (post-undo) pixels and can be replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightmapEntry {
    pub rect: DirtyRect,
    pub before: Vec<u16>,
}

impl HeightmapEntry {
    pub fn bytes(&self) -> usize {
        self.before.len() * std::mem::size_of::<u16>()
    }
}

/// One project-level diff. Each variant is reversible: on undo, the app
/// applies the *inverse* and pushes that inverse onto the redo stack;
/// on redo, the mirror. Inversion lives in the app because Wizard
/// snapshots cross the barme-core / barme-app boundary (project name +
/// height scale live on `App`, not `Project`).
///
/// **ADR-032 (B6):** position identifiers are `(ally_group_id, pos)`
/// not `team_id`. The flat `team_id` was a property of the
/// pre-Phase-3 model. Now a position is uniquely identified by the
/// group it lives in and its coordinates (coords are unique within a
/// group by construction — the F8 logic refuses duplicate placements).
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectDiff {
    /// A start position was added to `ally_group_id` at `pos`. Undo
    /// removes it; redo re-adds.
    PlaceStartPosition {
        ally_group_id: u8,
        pos: StartPosition,
    },
    /// A start position was deleted from `ally_group_id`. Undo
    /// re-adds; redo removes.
    DeleteStartPosition {
        ally_group_id: u8,
        pos: StartPosition,
    },
    /// A start position in `ally_group_id` was moved from `from` to
    /// `to`. Undo restores `from`; redo restores `to`. The drag
    /// finalizer collapses an entire drag gesture into one entry.
    MoveStartPosition {
        ally_group_id: u8,
        from: StartPosition,
        to: StartPosition,
    },
    /// The F1 wizard replaced project-level state wholesale. The boxed
    /// snapshot holds the *pre-wizard* state; on undo the app swaps it
    /// with the *current* (post-wizard) state and pushes the captured
    /// post-wizard snapshot onto the redo stack.
    ApplyWizard(Box<WizardSnapshot>),
    /// C4 (Sprint 11): a metal-spot source was added. Identity is the
    /// full `MetalSpot` record (coords + metal value). Undo removes
    /// the matching source; redo re-adds. Each LMB-click in
    /// `Tool::MetalSpots` produces one diff per source (symmetry
    /// mirrors each push their own diff so undo peels mirrors one at
    /// a time — matches F8 / B5 semantics).
    PlaceMetalSpot { spot: MetalSpot },
    /// A metal-spot source was deleted. Holds the full pre-delete
    /// record so undo can re-add it verbatim.
    DeleteMetalSpot { spot: MetalSpot },
    /// A metal-spot source's coordinates and/or metal value changed
    /// (drag-move on canvas, or DragValue edit in the inspector).
    /// `from` is the pre-edit record; `to` is post-edit. Identity is
    /// `from` for undo / `to` for redo. The drag finalizer collapses
    /// an entire drag gesture into a single entry.
    MoveMetalSpot { from: MetalSpot, to: MetalSpot },
    /// The `extractor_radius` global was edited. Symmetric inverse:
    /// undo restores `from`, redo restores `to`. Inspector edits
    /// collapse into one entry per commit (slider drag-release).
    SetExtractorRadius { from: f32, to: f32 },
    /// C5 (Sprint 11): a geo-vent source was added. See
    /// [`Self::PlaceMetalSpot`] for the symmetry / per-mirror diff
    /// semantics — identical.
    PlaceGeoVent { vent: GeoVent },
    /// A geo-vent source was deleted.
    DeleteGeoVent { vent: GeoVent },
    /// A geo-vent source's coordinates changed (drag-move / inspector
    /// edit).
    MoveGeoVent { from: GeoVent, to: GeoVent },
}

impl ProjectDiff {
    pub fn bytes(&self) -> usize {
        let inline = std::mem::size_of::<Self>();
        match self {
            ProjectDiff::PlaceStartPosition { .. }
            | ProjectDiff::DeleteStartPosition { .. }
            | ProjectDiff::MoveStartPosition { .. }
            | ProjectDiff::PlaceMetalSpot { .. }
            | ProjectDiff::DeleteMetalSpot { .. }
            | ProjectDiff::MoveMetalSpot { .. }
            | ProjectDiff::SetExtractorRadius { .. }
            | ProjectDiff::PlaceGeoVent { .. }
            | ProjectDiff::DeleteGeoVent { .. }
            | ProjectDiff::MoveGeoVent { .. } => inline,
            ProjectDiff::ApplyWizard(snap) => inline + snap.bytes(),
        }
    }
}

/// Project-level state replaced by an F1 wizard apply. Spans the few
/// app-level fields the wizard touches; the heightmap is deliberately
/// excluded (the wizard barriers the stroke history via
/// `apply_procgen()` so pre-wizard heightmap state is unrecoverable
/// anyway — restoring metadata + start positions is the achievable win).
#[derive(Debug, Clone, PartialEq)]
pub struct WizardSnapshot {
    pub project_name: String,
    pub map_size: MapSize,
    pub height_scale: f32,
    pub symmetry: SymmetryAxis,
    pub rotational_fold: u8,
    /// **ADR-032:** the wizard snapshot carries the full ally-group
    /// tree (the pre-Phase-3 flat vec is gone). Undo restores the
    /// entire tree wholesale, including colours, names, and box
    /// polygons.
    pub ally_groups: Vec<AllyGroup>,
    pub procgen_expr: String,
    pub procgen_domain: Domain,
}

impl WizardSnapshot {
    /// Bytes the heap-owned fields occupy. Used by the cap eviction.
    pub fn bytes(&self) -> usize {
        let positions_bytes: usize = self
            .ally_groups
            .iter()
            .map(|g| {
                g.start_positions.capacity() * std::mem::size_of::<StartPosition>()
                    + g.name.capacity()
                    + g.box_polygon
                        .as_ref()
                        .map(|p| p.capacity() * std::mem::size_of::<(f32, f32)>())
                        .unwrap_or(0)
            })
            .sum();
        let name_bytes = self.project_name.capacity();
        let expr_bytes = self.procgen_expr.capacity();
        let groups_bytes = self.ally_groups.capacity() * std::mem::size_of::<AllyGroup>();
        positions_bytes + name_bytes + expr_bytes + groups_bytes
    }
}

/// In-flight heightmap stroke state. Owned by [`History`]; not exposed.
struct OpenStroke {
    width: u32,
    height: u32,
    scratch: Vec<u16>,
    snapped: Vec<u64>,
}

impl OpenStroke {
    fn new(width: u32, height: u32) -> Self {
        let pixels = (width as usize) * (height as usize);
        let words = pixels.div_ceil(64);
        Self {
            width,
            height,
            scratch: vec![0u16; pixels],
            snapped: vec![0u64; words],
        }
    }

    fn matches(&self, hm: &Heightmap) -> bool {
        self.width == hm.width() && self.height == hm.height()
    }

    fn snapshot_rect(&mut self, hm: &Heightmap, rect: DirtyRect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        debug_assert!(rect.x + rect.w <= self.width);
        debug_assert!(rect.y + rect.h <= self.height);
        let stride = self.width as usize;
        let src = hm.data();
        for row in 0..rect.h {
            let y = (rect.y + row) as usize;
            for col in 0..rect.w {
                let x = (rect.x + col) as usize;
                let idx = y * stride + x;
                let word = idx / 64;
                let bit = idx % 64;
                let mask = 1u64 << bit;
                if (self.snapped[word] & mask) == 0 {
                    self.scratch[idx] = src[idx];
                    self.snapped[word] |= mask;
                }
            }
        }
    }

    fn snapped_bbox(&self) -> Option<DirtyRect> {
        let stride = self.width as usize;
        let total = stride * (self.height as usize);
        let mut min_x = u32::MAX;
        let mut min_y = u32::MAX;
        let mut max_x = 0u32;
        let mut max_y = 0u32;
        let mut any = false;
        for (word_idx, &word) in self.snapped.iter().enumerate() {
            if word == 0 {
                continue;
            }
            let base = word_idx * 64;
            let mut w = word;
            while w != 0 {
                let bit = w.trailing_zeros() as usize;
                let idx = base + bit;
                if idx < total {
                    let y = (idx / stride) as u32;
                    let x = (idx % stride) as u32;
                    if x < min_x {
                        min_x = x;
                    }
                    if x > max_x {
                        max_x = x;
                    }
                    if y < min_y {
                        min_y = y;
                    }
                    if y > max_y {
                        max_y = y;
                    }
                    any = true;
                }
                w &= w - 1;
            }
        }
        if !any {
            return None;
        }
        Some(DirtyRect {
            x: min_x,
            y: min_y,
            w: max_x - min_x + 1,
            h: max_y - min_y + 1,
        })
    }

    fn build_before(&self, hm: &Heightmap, rect: DirtyRect) -> Vec<u16> {
        let stride = self.width as usize;
        let src = hm.data();
        let mut out = Vec::with_capacity((rect.w as usize) * (rect.h as usize));
        for row in 0..rect.h {
            let y = (rect.y + row) as usize;
            for col in 0..rect.w {
                let x = (rect.x + col) as usize;
                let idx = y * stride + x;
                let word = idx / 64;
                let bit = idx % 64;
                let mask = 1u64 << bit;
                if (self.snapped[word] & mask) != 0 {
                    out.push(self.scratch[idx]);
                } else {
                    out.push(src[idx]);
                }
            }
        }
        out
    }
}

/// Bounded undo/redo stack. The cap is enforced on every push by evicting
/// the *largest* committed entry until total bytes ≤ `cap_bytes`. Default
/// cap is [`History::DEFAULT_CAP_BYTES`].
pub struct History {
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    bytes: usize,
    cap_bytes: usize,
    open: Option<OpenStroke>,
}

impl History {
    /// 100 MB — > 50 long strokes of headroom on a 16-SMU map plus
    /// hundreds of ProjectDiff entries.
    pub const DEFAULT_CAP_BYTES: usize = 100 * 1024 * 1024;

    pub fn new(cap_bytes: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            bytes: 0,
            cap_bytes,
            open: None,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn cap_bytes(&self) -> usize {
        self.cap_bytes
    }

    /// `true` between the first [`History::snapshot_rect`] of a stroke
    /// and the matching [`History::end_stroke`].
    pub fn stroke_open(&self) -> bool {
        self.open.is_some()
    }

    /// Drop the entire history AND any in-flight stroke. Called when the
    /// project state is replaced wholesale (procgen, load, new project).
    pub fn barrier(&mut self) {
        let had_open = self.open.is_some();
        if !self.undo.is_empty() || !self.redo.is_empty() || had_open {
            trace!(
                undo = self.undo.len(),
                redo = self.redo.len(),
                bytes = self.bytes,
                open_stroke = had_open,
                "undo barrier: clearing history"
            );
        }
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
        self.open = None;
    }

    // ───────── heightmap-stroke capture (unchanged from ADR-033) ─────────

    /// Capture pre-edit values for novel pixels in `rect`. Lazy: opens a
    /// stroke on the first call. Must be invoked **before** the brush
    /// writes to the heightmap so the captured values are pre-stroke.
    pub fn snapshot_rect(&mut self, hm: &Heightmap, rect: DirtyRect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        let stroke = match self.open.as_mut() {
            Some(s) if s.matches(hm) => s,
            Some(_) => {
                warn!("undo: heightmap dims changed mid-stroke; restarting stroke");
                self.open = Some(OpenStroke::new(hm.width(), hm.height()));
                self.open.as_mut().unwrap()
            }
            None => {
                self.open = Some(OpenStroke::new(hm.width(), hm.height()));
                self.open.as_mut().unwrap()
            }
        };
        stroke.snapshot_rect(hm, rect);
    }

    /// Commit the in-flight stroke. Builds one `Heightmap` entry covering
    /// the unioned bbox of snapshotted pixels and pushes it; no-op if no
    /// stroke is open or the bitset is empty.
    pub fn end_stroke(&mut self, hm: &Heightmap) {
        let Some(stroke) = self.open.take() else {
            return;
        };
        let Some(rect) = stroke.snapped_bbox() else {
            return;
        };
        let before = stroke.build_before(hm, rect);
        let entry = HistoryEntry::Heightmap(HeightmapEntry { rect, before });
        let added = entry.bytes();
        trace!(
            rect = ?(rect.x, rect.y, rect.w, rect.h),
            bytes = added,
            "stroke committed to undo history"
        );
        self.redo.clear();
        self.bytes = self.bytes.saturating_add(added);
        self.undo.push_back(entry);
        self.evict_until_under_cap();
    }

    // ───────── project-diff capture (B5) ─────────

    /// Push a project-level diff. Clears the redo stack (linear history,
    /// same as heightmap strokes) and enforces the cap.
    pub fn push_project_diff(&mut self, diff: ProjectDiff) {
        let entry = HistoryEntry::Project(diff);
        let added = entry.bytes();
        trace!(
            bytes = added,
            kind = ?std::mem::discriminant(&entry),
            "project diff committed to undo history"
        );
        self.redo.clear();
        self.bytes = self.bytes.saturating_add(added);
        self.undo.push_back(entry);
        self.evict_until_under_cap();
    }

    // ───────── pop / push dispatch ─────────

    /// Pop the top of the undo stack. Caller MUST follow up with
    /// [`History::push_to_redo`] using whatever entry restores the
    /// post-pop state — otherwise the redo stack drifts.
    pub fn pop_undo(&mut self) -> Option<HistoryEntry> {
        let entry = self.undo.pop_back()?;
        self.bytes = self.bytes.saturating_sub(entry.bytes());
        Some(entry)
    }

    /// Pop the top of the redo stack. Caller MUST follow up with
    /// [`History::push_to_undo`].
    pub fn pop_redo(&mut self) -> Option<HistoryEntry> {
        let entry = self.redo.pop()?;
        Some(entry)
    }

    /// Push onto the redo stack — used by `undo_one` after applying an
    /// entry, with the *inverse* of what was applied.
    pub fn push_to_redo(&mut self, entry: HistoryEntry) {
        self.redo.push(entry);
        // Redo entries don't accrue bytes against the cap. The cap
        // governs the undo stack; redo is bounded structurally by the
        // user's undo depth and gets cleared on the next push anyway.
    }

    /// Push onto the undo stack without clearing redo. Used by
    /// `redo_one` after applying an entry.
    pub fn push_to_undo(&mut self, entry: HistoryEntry) {
        let added = entry.bytes();
        self.bytes = self.bytes.saturating_add(added);
        self.undo.push_back(entry);
        self.evict_until_under_cap();
    }

    /// Apply the swap encoded by `entry` against `hm`. After this returns
    /// `entry` holds the *post-swap* pixels and can be pushed onto the
    /// opposite stack as the inverse. Returns the affected rect for the
    /// caller's GPU sub-upload.
    pub fn apply_heightmap(&self, entry: &mut HeightmapEntry, hm: &mut Heightmap) -> DirtyRect {
        hm.swap_rect(
            entry.rect.x,
            entry.rect.y,
            entry.rect.w,
            entry.rect.h,
            &mut entry.before,
        );
        entry.rect
    }

    // ───────── cap enforcement ─────────

    /// Evict the *largest* committed undo entry until total bytes are at
    /// or under cap. Largest-first (rather than oldest-first) keeps a
    /// single 2 MB stroke from evicting 20 kilobyte-sized ProjectDiff
    /// entries on every long sculpt.
    fn evict_until_under_cap(&mut self) {
        while self.bytes > self.cap_bytes && !self.undo.is_empty() {
            // VecDeque has no random-access remove that's both safe and
            // cheap; for the realistic 50-100 entry depth, a linear
            // scan is comfortably sub-microsecond.
            let mut largest_idx = 0;
            let mut largest_bytes = self.undo[0].bytes();
            for (i, e) in self.undo.iter().enumerate().skip(1) {
                let b = e.bytes();
                if b > largest_bytes {
                    largest_bytes = b;
                    largest_idx = i;
                }
            }
            let evicted = self
                .undo
                .remove(largest_idx)
                .expect("idx came from iter; must exist");
            self.bytes = self.bytes.saturating_sub(evicted.bytes());
            warn!(
                bytes_evicted = evicted.bytes(),
                bytes_remaining = self.bytes,
                cap_bytes = self.cap_bytes,
                evicted_idx = largest_idx,
                undo_depth = self.undo.len(),
                "undo: history over cap; evicted largest entry"
            );
        }
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAP_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    fn mk_hm() -> Heightmap {
        Heightmap::synth_ramp(MapSize::square(2)) // 129×129
    }

    /// Helper: snapshot `rect`, then write `value` into the heightmap at
    /// every pixel of `rect`. Mirrors what `apply_brush_at` does in
    /// `main.rs`.
    fn write_stamp(history: &mut History, hm: &mut Heightmap, rect: DirtyRect, value: u16) {
        history.snapshot_rect(hm, rect);
        let stride = hm.width() as usize;
        for row in 0..rect.h {
            for col in 0..rect.w {
                let idx = ((rect.y + row) as usize) * stride + (rect.x + col) as usize;
                hm.data_mut()[idx] = value;
            }
        }
    }

    /// Walk the unified undo stack to completion, applying each entry
    /// against `hm` and the supplied group's positions. Used by the
    /// "stroke + 50 placements -> 51 walkback" test to mirror the
    /// app-side dispatcher without pulling main.rs into barme-core.
    fn drain_undo_to_state(history: &mut History, hm: &mut Heightmap, group: &mut AllyGroup) {
        while let Some(entry) = history.pop_undo() {
            match entry {
                HistoryEntry::Heightmap(mut e) => {
                    history.apply_heightmap(&mut e, hm);
                    history.push_to_redo(HistoryEntry::Heightmap(e));
                }
                HistoryEntry::Project(diff) => {
                    let inverse = apply_project_diff_for_test(diff, group);
                    history.push_to_redo(HistoryEntry::Project(inverse));
                }
            }
        }
    }

    /// Test-only project-diff dispatcher mirroring the app-side one.
    /// Operates on a single ally group's `start_positions`; the app
    /// dispatches by `ally_group_id` and forwards to the matching
    /// group in `Project.ally_groups`.
    fn apply_project_diff_for_test(diff: ProjectDiff, group: &mut AllyGroup) -> ProjectDiff {
        match diff {
            ProjectDiff::PlaceStartPosition { ally_group_id, pos } => {
                assert_eq!(ally_group_id, group.id);
                group.start_positions.retain(|q| *q != pos);
                ProjectDiff::DeleteStartPosition { ally_group_id, pos }
            }
            ProjectDiff::DeleteStartPosition { ally_group_id, pos } => {
                assert_eq!(ally_group_id, group.id);
                group.start_positions.push(pos);
                ProjectDiff::PlaceStartPosition { ally_group_id, pos }
            }
            ProjectDiff::MoveStartPosition {
                ally_group_id,
                from,
                to,
            } => {
                assert_eq!(ally_group_id, group.id);
                if let Some(p) = group.start_positions.iter_mut().find(|p| **p == to) {
                    *p = from;
                }
                ProjectDiff::MoveStartPosition {
                    ally_group_id,
                    from: to,
                    to: from,
                }
            }
            ProjectDiff::ApplyWizard(_) => unreachable!("not exercised in this test set"),
            // C4/C5 variants aren't dispatched against AllyGroup —
            // they target Project's metal_spots / geo_vents lists,
            // which live outside this helper's scope. The pinned
            // unit tests for these variants use the bytes() /
            // history-stack checks instead.
            ProjectDiff::PlaceMetalSpot { .. }
            | ProjectDiff::DeleteMetalSpot { .. }
            | ProjectDiff::MoveMetalSpot { .. }
            | ProjectDiff::SetExtractorRadius { .. }
            | ProjectDiff::PlaceGeoVent { .. }
            | ProjectDiff::DeleteGeoVent { .. }
            | ProjectDiff::MoveGeoVent { .. } => {
                unreachable!("metal/geo diffs not exercised through AllyGroup test helper")
            }
        }
    }

    #[test]
    fn round_trip_single_stamp() {
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();
        let mut h = History::default();

        let rect = DirtyRect {
            x: 10,
            y: 10,
            w: 5,
            h: 5,
        };
        write_stamp(&mut h, &mut hm, rect, 0xDEAD);
        h.end_stroke(&hm);
        assert!(h.can_undo());

        // Undo restores the original pixels.
        let mut entry = h.pop_undo().unwrap();
        let r = if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm)
        } else {
            panic!("expected Heightmap entry");
        };
        assert_eq!(r, rect);
        assert_eq!(hm.data(), &orig[..]);
        h.push_to_redo(entry);
        assert!(h.can_redo() && !h.can_undo());

        // Redo restores the edit.
        let mut entry = h.pop_redo().unwrap();
        let r2 = if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm)
        } else {
            panic!("expected Heightmap entry");
        };
        assert_eq!(r2, rect);
        let idx = (10 * hm.width() + 10) as usize;
        assert_eq!(hm.data()[idx], 0xDEAD);
        h.push_to_undo(entry);
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn overlapping_stamps_in_one_stroke_revert_to_pre_stroke_state() {
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();
        let mut h = History::default();

        let rect_a = DirtyRect {
            x: 10,
            y: 10,
            w: 4,
            h: 4,
        };
        let rect_b = DirtyRect {
            x: 12,
            y: 12,
            w: 4,
            h: 4,
        };
        write_stamp(&mut h, &mut hm, rect_a, 0xAAAA);
        write_stamp(&mut h, &mut hm, rect_b, 0xBBBB);
        h.end_stroke(&hm);

        let mut entry = h.pop_undo().unwrap();
        let r = if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm)
        } else {
            panic!("expected Heightmap entry");
        };
        // Bbox is the union: (10,10) to (15,15) inclusive → 6×6.
        assert_eq!(
            r,
            DirtyRect {
                x: 10,
                y: 10,
                w: 6,
                h: 6
            }
        );
        assert_eq!(hm.data(), &orig[..], "pre-stroke state must be restored");
    }

    #[test]
    fn snapshot_is_bounded_by_unioned_bbox_not_stamp_count() {
        let mut hm = mk_hm();
        let mut h = History::default();

        let rect = DirtyRect {
            x: 50,
            y: 50,
            w: 16,
            h: 16,
        };
        for _ in 0..120 {
            write_stamp(&mut h, &mut hm, rect, 0xFEED);
        }
        h.end_stroke(&hm);

        let entry = h.undo.back().unwrap();
        match entry {
            HistoryEntry::Heightmap(e) => {
                assert_eq!(e.rect, rect);
                assert_eq!(e.bytes(), 16 * 16 * 2);
            }
            _ => panic!("expected Heightmap entry"),
        }
    }

    #[test]
    fn snapshot_120_overlapping_stamps_stays_under_5x_affected_pixel_bytes() {
        let mut hm = Heightmap::synth_ramp(MapSize::square(4)); // 257×257
        let mut h = History::default();
        let stamp_w = 16u32;
        for i in 0..120u32 {
            let rect = DirtyRect {
                x: i,
                y: i,
                w: stamp_w,
                h: stamp_w,
            };
            write_stamp(&mut h, &mut hm, rect, 0xCAFE);
        }
        h.end_stroke(&hm);

        let entry = h.undo.back().unwrap();
        let HistoryEntry::Heightmap(e) = entry else {
            panic!("expected Heightmap entry")
        };
        let expected = (e.rect.w as usize) * (e.rect.h as usize) * 2;
        assert_eq!(e.bytes(), expected);
        assert!(e.bytes() < 1_000_000, "entry too large: {}", e.bytes());
    }

    #[test]
    fn snapshot_then_undo_byte_identical_to_pre_stroke() {
        let mut hm = Heightmap::synth_ramp(MapSize::square(2));
        let orig = hm.data().to_vec();
        let mut h = History::default();

        for (x, y, w, side) in [(5, 5, 8, 8u32), (20, 6, 4, 4), (8, 10, 6, 6)] {
            write_stamp(&mut h, &mut hm, DirtyRect { x, y, w, h: side }, 0xBEEF);
        }
        h.end_stroke(&hm);

        let mut entry = h.pop_undo().unwrap();
        if let HistoryEntry::Heightmap(ref mut e) = entry {
            h.apply_heightmap(e, &mut hm);
        }
        assert_eq!(hm.data(), &orig[..]);
    }

    #[test]
    fn new_heightmap_edit_clears_redo() {
        let mut hm = mk_hm();
        let mut h = History::default();
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            0x1111,
        );
        h.end_stroke(&hm);
        let mut e = h.pop_undo().unwrap();
        if let HistoryEntry::Heightmap(ref mut he) = e {
            h.apply_heightmap(he, &mut hm);
        }
        h.push_to_redo(e);
        assert!(h.can_redo());

        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 5,
                y: 5,
                w: 2,
                h: 2,
            },
            0x2222,
        );
        h.end_stroke(&hm);
        assert!(!h.can_redo(), "new push must clear redo stack");
    }

    #[test]
    fn new_project_diff_clears_redo() {
        let mut h = History::default();
        let pos = StartPosition {
            x_elmo: 100,
            z_elmo: 100,
        };
        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });
        let popped = h.pop_undo().unwrap();
        h.push_to_redo(popped);
        assert!(h.can_redo());

        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });
        assert!(!h.can_redo(), "new project-diff push must clear redo stack");
    }

    #[test]
    fn barrier_clears_everything_including_open_stroke() {
        let mut hm = mk_hm();
        let mut h = History::default();
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            0x3333,
        );
        h.end_stroke(&hm);
        let mut e = h.pop_undo().unwrap();
        if let HistoryEntry::Heightmap(ref mut he) = e {
            h.apply_heightmap(he, &mut hm);
        }
        h.push_to_redo(e);

        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos: StartPosition {
                x_elmo: 50,
                z_elmo: 50,
            },
        });

        // Open a fresh stroke without committing.
        h.snapshot_rect(
            &hm,
            DirtyRect {
                x: 8,
                y: 8,
                w: 2,
                h: 2,
            },
        );
        assert!(h.stroke_open());

        h.barrier();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
        assert!(!h.stroke_open(), "barrier must drop the open stroke");
        assert_eq!(h.bytes(), 0);
    }

    #[test]
    fn end_stroke_with_no_snapshots_is_noop() {
        let hm = mk_hm();
        let mut h = History::default();
        h.end_stroke(&hm);
        assert!(!h.can_undo());

        h.snapshot_rect(
            &hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
        );
        h.end_stroke(&hm);
        assert!(!h.can_undo());
    }

    #[test]
    fn cap_evicts_largest_not_oldest() {
        // Push: small project diff (oldest), then a 'large' stroke.
        // Pre-B5 the oldest (the diff) would have been evicted; B5
        // evicts the largest (the stroke).
        let mut hm = mk_hm();
        let mut h = History::new(40); // small cap to force eviction

        let pos = StartPosition {
            x_elmo: 1,
            z_elmo: 1,
        };
        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });
        let small_bytes = h.bytes();

        // Now push a heightmap entry larger than the project diff.
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 6,
                h: 6,
            },
            0xF00D,
        );
        h.end_stroke(&hm);

        // Eviction should kick out the largest (the heightmap entry,
        // 72 bytes) — the small ProjectDiff survives.
        assert!(h.bytes() <= 40);
        assert_eq!(h.undo_depth(), 1);
        let surviving = h.undo.back().unwrap();
        match surviving {
            HistoryEntry::Project(ProjectDiff::PlaceStartPosition {
                ally_group_id,
                pos: p2,
            }) => {
                assert_eq!(*ally_group_id, 0);
                assert_eq!(p2.x_elmo, 1);
            }
            _ => panic!("largest entry should have been evicted, got {surviving:?}"),
        }
        let _ = small_bytes;
    }

    #[test]
    fn redo_after_undo_chain_heightmap() {
        let mut hm = mk_hm();
        let mut h = History::default();

        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 4,
                h: 4,
            },
            0x1111,
        );
        h.end_stroke(&hm);
        write_stamp(
            &mut h,
            &mut hm,
            DirtyRect {
                x: 4,
                y: 4,
                w: 4,
                h: 4,
            },
            0x2222,
        );
        h.end_stroke(&hm);

        let final_state = hm.data().to_vec();
        // Undo twice.
        for _ in 0..2 {
            let mut e = h.pop_undo().unwrap();
            if let HistoryEntry::Heightmap(ref mut he) = e {
                h.apply_heightmap(he, &mut hm);
            }
            h.push_to_redo(e);
        }
        // Redo twice.
        for _ in 0..2 {
            let mut e = h.pop_redo().unwrap();
            if let HistoryEntry::Heightmap(ref mut he) = e {
                h.apply_heightmap(he, &mut hm);
            }
            h.push_to_undo(e);
        }
        assert_eq!(hm.data(), &final_state[..]);
    }

    // ─────── B5 project-diff tests ───────

    #[test]
    fn place_start_position_round_trip() {
        let mut h = History::default();
        let mut group = AllyGroup::new(0);
        let pos = StartPosition {
            x_elmo: 100,
            z_elmo: 200,
        };
        group.start_positions.push(pos);
        h.push_project_diff(ProjectDiff::PlaceStartPosition {
            ally_group_id: 0,
            pos,
        });

        // Undo: remove pos.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            panic!("expected Project entry");
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        h.push_to_redo(HistoryEntry::Project(inverse));
        assert!(
            group.start_positions.is_empty(),
            "undo should remove the placed position"
        );

        // Redo: re-place.
        let entry = h.pop_redo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            panic!("expected Project entry");
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        h.push_to_undo(HistoryEntry::Project(inverse));
        assert_eq!(group.start_positions.len(), 1);
        assert_eq!(group.start_positions[0], pos);
    }

    #[test]
    fn move_start_position_round_trip() {
        let mut h = History::default();
        let from = StartPosition {
            x_elmo: 500,
            z_elmo: 600,
        };
        let to = StartPosition {
            x_elmo: 1500,
            z_elmo: 1600,
        };
        let mut group = AllyGroup {
            start_positions: vec![to],
            ..AllyGroup::new(0)
        };
        // App pretends the marker moved from `from` to `to`.
        h.push_project_diff(ProjectDiff::MoveStartPosition {
            ally_group_id: 0,
            from,
            to,
        });

        // Undo.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        assert_eq!(group.start_positions[0], from);
        h.push_to_redo(HistoryEntry::Project(inverse));

        // Redo.
        let entry = h.pop_redo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        apply_project_diff_for_test(diff, &mut group);
        assert_eq!(group.start_positions[0], to);
    }

    #[test]
    fn delete_start_position_round_trip() {
        let mut h = History::default();
        let pos = StartPosition {
            x_elmo: 99,
            z_elmo: 99,
        };
        let mut group = AllyGroup::new(0);
        // App removed pos already; record the diff.
        h.push_project_diff(ProjectDiff::DeleteStartPosition {
            ally_group_id: 0,
            pos,
        });

        // Undo: pos returns.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        let inverse = apply_project_diff_for_test(diff, &mut group);
        assert_eq!(group.start_positions.len(), 1);
        h.push_to_redo(HistoryEntry::Project(inverse));

        // Redo: pos goes away again.
        let entry = h.pop_redo().unwrap();
        let HistoryEntry::Project(diff) = entry else {
            unreachable!()
        };
        apply_project_diff_for_test(diff, &mut group);
        assert!(group.start_positions.is_empty());
    }

    /// Phase-3-plan smoke: one long stroke + 50 placements + Ctrl-Z×51
    /// returns to pre-stroke state.
    #[test]
    fn stroke_then_50_placements_walks_back_in_51_steps() {
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();
        let mut group = AllyGroup::new(0);
        let mut h = History::default();

        // 1. Long stroke — 80 overlapping stamps along a diagonal.
        for i in 0..80u32 {
            write_stamp(
                &mut h,
                &mut hm,
                DirtyRect {
                    x: i,
                    y: i,
                    w: 8,
                    h: 8,
                },
                0xBEEF,
            );
        }
        h.end_stroke(&hm);
        assert_eq!(h.undo_depth(), 1);

        // 2. 50 start-position placements.
        for i in 0..50i32 {
            let pos = StartPosition {
                x_elmo: i * 10,
                z_elmo: i * 10,
            };
            group.start_positions.push(pos);
            h.push_project_diff(ProjectDiff::PlaceStartPosition {
                ally_group_id: 0,
                pos,
            });
        }
        assert_eq!(h.undo_depth(), 51);
        assert_eq!(group.start_positions.len(), 50);

        // 3. Walk all 51 back. After each pop the heightmap or position
        //    list mutates. After 51 pops we expect pre-stroke state.
        drain_undo_to_state(&mut h, &mut hm, &mut group);

        assert!(
            group.start_positions.is_empty(),
            "all 50 positions should be cleared"
        );
        assert_eq!(hm.data(), &orig[..], "heightmap should be pre-stroke");
        assert!(!h.can_undo(), "stack should be empty");
        assert_eq!(h.bytes(), 0);
        assert_eq!(h.redo_depth(), 51, "all 51 entries on the redo side");
    }

    #[test]
    fn apply_wizard_snapshot_round_trip() {
        let mut h = History::default();
        let mut g = AllyGroup::new(0);
        g.start_positions.push(StartPosition {
            x_elmo: 1,
            z_elmo: 1,
        });
        let snap = WizardSnapshot {
            project_name: "pre-wizard".to_string(),
            map_size: MapSize::square(8),
            height_scale: 256.0,
            symmetry: SymmetryAxis::None,
            rotational_fold: 2,
            ally_groups: vec![g],
            procgen_expr: "x".to_string(),
            procgen_domain: Domain::Unit,
        };
        h.push_project_diff(ProjectDiff::ApplyWizard(Box::new(snap.clone())));
        assert_eq!(h.undo_depth(), 1);

        // Pop and inspect.
        let entry = h.pop_undo().unwrap();
        let HistoryEntry::Project(ProjectDiff::ApplyWizard(boxed)) = entry else {
            panic!("expected ApplyWizard");
        };
        assert_eq!(*boxed, snap);
    }

    /// C4 (Sprint 11): metal-spot diffs are inline (no heap),
    /// matching `PlaceStartPosition`. Verify they account for bytes
    /// the same way under cap pressure.
    #[test]
    fn metal_spot_diff_bytes_match_inline_enum_size() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let place = ProjectDiff::PlaceMetalSpot {
            spot: MetalSpot::new(100, 200),
        };
        let delete = ProjectDiff::DeleteMetalSpot {
            spot: MetalSpot::new(100, 200),
        };
        let m = ProjectDiff::MoveMetalSpot {
            from: MetalSpot::new(100, 100),
            to: MetalSpot::new(200, 200),
        };
        assert_eq!(place.bytes(), inline);
        assert_eq!(delete.bytes(), inline);
        assert_eq!(m.bytes(), inline);
    }

    /// Same for geo-vent diffs.
    #[test]
    fn geo_vent_diff_bytes_match_inline_enum_size() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let place = ProjectDiff::PlaceGeoVent {
            vent: GeoVent::new(100, 200),
        };
        let delete = ProjectDiff::DeleteGeoVent {
            vent: GeoVent::new(100, 200),
        };
        let m = ProjectDiff::MoveGeoVent {
            from: GeoVent::new(100, 100),
            to: GeoVent::new(200, 200),
        };
        assert_eq!(place.bytes(), inline);
        assert_eq!(delete.bytes(), inline);
        assert_eq!(m.bytes(), inline);
    }

    /// Setting extractor_radius is also bytes-inline.
    #[test]
    fn extractor_radius_diff_is_inline() {
        let inline = std::mem::size_of::<ProjectDiff>();
        let d = ProjectDiff::SetExtractorRadius {
            from: 80.0,
            to: 120.0,
        };
        assert_eq!(d.bytes(), inline);
    }

    /// C4/C5 diff variants push through history just like the F8
    /// variants — sanity check the stack accounting and redo
    /// clearance on a metal-spot place.
    #[test]
    fn place_metal_spot_clears_redo_and_grows_undo() {
        let mut h = History::default();
        let spot = MetalSpot::new(1, 2);
        h.push_project_diff(ProjectDiff::PlaceMetalSpot { spot });
        let popped = h.pop_undo().unwrap();
        h.push_to_redo(popped);
        assert!(h.can_redo());

        // A new push must clear redo, same as the F8 path.
        h.push_project_diff(ProjectDiff::PlaceGeoVent {
            vent: GeoVent::new(3, 4),
        });
        assert!(!h.can_redo());
    }

    #[test]
    fn cap_eviction_evicts_largest_under_pressure() {
        // 100 small project diffs followed by one huge heightmap stroke.
        // Cap is tight enough to force eviction; we should lose the
        // heightmap entry, not the small diffs.
        let mut hm = mk_hm();
        let mut h = History::new(10_000);
        for i in 0..100i32 {
            h.push_project_diff(ProjectDiff::PlaceStartPosition {
                ally_group_id: 0,
                pos: StartPosition {
                    x_elmo: i,
                    z_elmo: i,
                },
            });
        }
        let depth_before = h.undo_depth();
        // ~50×50 stamp = 50*50*2 = 5000 bytes. Push another 5000-byte
        // entry: total would be 10000 + diffs (~few kb). Push a 6000-
        // byte stroke to force eviction.
        let mut h2 = h;
        write_stamp(
            &mut h2,
            &mut hm,
            DirtyRect {
                x: 0,
                y: 0,
                w: 80,
                h: 80,
            },
            0xF00D,
        );
        h2.end_stroke(&hm);
        // The heightmap entry is the largest by far (80*80*2 = 12800).
        // If 12800 > cap by itself, ALL entries evict — but our diffs
        // are much smaller (~32 bytes each), so the eviction should
        // first pop the heightmap entry alone.
        assert!(h2.bytes() <= h2.cap_bytes());
        assert_eq!(h2.undo_depth(), depth_before, "all small diffs survive");
        for e in h2.undo.iter() {
            assert!(matches!(e, HistoryEntry::Project(_)));
        }
    }
}
