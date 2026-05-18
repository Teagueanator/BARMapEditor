//! Undo / redo over heightmap dirty-rect snapshots (ADR-022).
//!
//! The unit of history is an [`UndoEntry`] — one *stroke*, where a stroke is
//! everything the user did between LMB-down and LMB-up. A single pointer
//! event during a stroke contributes one [`StampSnapshot`] (the union of all
//! symmetric stamps in that frame plus the pre-edit pixel bytes).
//!
//! Applying an undo is a slice-level swap: the snapshot's `before` buffer is
//! swapped with the current pixels in the heightmap, so the popped entry now
//! holds the *after* state and can be re-pushed onto the redo stack. Redo is
//! the same operation in reverse. Both run in `O(rect_area)` per stamp; no
//! cloning beyond the initial pre-edit capture.
//!
//! Barriers — procgen apply, heightmap load, new project — clear both stacks
//! rather than try to capture a 2 MB full-map diff (`64·N+1`²·2 bytes). The
//! UX rationale is in ADR-022.

use std::collections::VecDeque;

use tracing::{trace, warn};

use crate::brushes::DirtyRect;
use crate::heightmap::Heightmap;

/// Pre-edit pixels for one dirty rect. `before` is row-major, `rect.w * rect.h`
/// samples. After [`History::apply_undo`] / [`History::apply_redo`] swaps, the
/// same snapshot holds the *current* pixels and can be replayed.
#[derive(Debug, Clone)]
pub struct StampSnapshot {
    pub rect: DirtyRect,
    pub before: Vec<u16>,
}

impl StampSnapshot {
    pub fn bytes(&self) -> usize {
        self.before.len() * std::mem::size_of::<u16>()
    }
}

/// One stroke worth of stamps. Stamps are stored in apply order; undo walks
/// them in reverse so each pixel ends up at its pre-stroke value even when
/// rects overlap (later stamps wrote on top of earlier ones).
#[derive(Debug, Clone, Default)]
pub struct UndoEntry {
    pub stamps: Vec<StampSnapshot>,
}

impl UndoEntry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, snap: StampSnapshot) {
        self.stamps.push(snap);
    }

    pub fn is_empty(&self) -> bool {
        self.stamps.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.stamps.iter().map(StampSnapshot::bytes).sum()
    }

    pub fn stamp_count(&self) -> usize {
        self.stamps.len()
    }
}

/// Bounded undo/redo stack. Memory cap is enforced on push by evicting the
/// oldest entry; default cap is `History::DEFAULT_CAP_BYTES`.
pub struct History {
    undo: VecDeque<UndoEntry>,
    redo: Vec<UndoEntry>,
    bytes: usize,
    cap_bytes: usize,
}

impl History {
    /// 100 MB — > 100 strokes of headroom at typical brush radius on a 16×16 SMU map.
    pub const DEFAULT_CAP_BYTES: usize = 100 * 1024 * 1024;

    pub fn new(cap_bytes: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            bytes: 0,
            cap_bytes,
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

    /// Drop the entire history. Called when the heightmap is replaced
    /// wholesale (procgen, load, new project) — those events would otherwise
    /// require snapshotting the full pre-state, and undoing across them is
    /// confusing UX.
    pub fn barrier(&mut self) {
        if !self.undo.is_empty() || !self.redo.is_empty() {
            trace!(
                undo = self.undo.len(),
                redo = self.redo.len(),
                bytes = self.bytes,
                "undo barrier: clearing history"
            );
        }
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
    }

    /// Commit a finished stroke. Clears the redo stack (linear history).
    /// Evicts oldest entries until total size ≤ `cap_bytes`.
    pub fn push(&mut self, entry: UndoEntry) {
        if entry.is_empty() {
            return;
        }
        let added = entry.bytes();
        self.redo.clear();
        self.bytes = self.bytes.saturating_add(added);
        self.undo.push_back(entry);
        while self.bytes > self.cap_bytes
            && let Some(evicted) = self.undo.pop_front()
        {
            let n = evicted.bytes();
            self.bytes = self.bytes.saturating_sub(n);
            warn!(
                bytes_evicted = n,
                bytes_remaining = self.bytes,
                cap_bytes = self.cap_bytes,
                "undo: history over cap; evicted oldest entry"
            );
        }
    }

    /// Pop the most recent stroke and apply it to `hm`, returning the union
    /// of all dirty rects that need GPU re-upload. The popped entry — now
    /// holding the *current* (pre-undo) pixels — is pushed onto the redo
    /// stack so a subsequent redo replays it.
    pub fn apply_undo(&mut self, hm: &mut Heightmap) -> Option<DirtyRect> {
        let mut entry = self.undo.pop_back()?;
        let n = entry.bytes();
        self.bytes = self.bytes.saturating_sub(n);
        let rect = swap_entry(&mut entry, hm, /*reverse=*/ true);
        self.redo.push(entry);
        rect
    }

    /// Pop the most recent redo and apply it to `hm`. Mirror of `apply_undo`:
    /// the popped entry is pushed back onto the undo stack with the now-
    /// inverted snapshots.
    pub fn apply_redo(&mut self, hm: &mut Heightmap) -> Option<DirtyRect> {
        let mut entry = self.redo.pop()?;
        let rect = swap_entry(&mut entry, hm, /*reverse=*/ false);
        let n = entry.bytes();
        self.bytes = self.bytes.saturating_add(n);
        self.undo.push_back(entry);
        rect
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAP_BYTES)
    }
}

/// Swap each stamp's `before` buffer with the corresponding heightmap rect.
/// When `reverse` is true (undo) stamps are walked back-to-front so overlapping
/// later edits are reverted before the earlier ones they wrote over.
fn swap_entry(entry: &mut UndoEntry, hm: &mut Heightmap, reverse: bool) -> Option<DirtyRect> {
    let mut union: Option<DirtyRect> = None;
    let stamps = entry.stamps.as_mut_slice();
    let indices: Box<dyn Iterator<Item = usize>> = if reverse {
        Box::new((0..stamps.len()).rev())
    } else {
        Box::new(0..stamps.len())
    };
    for i in indices {
        let snap = &mut stamps[i];
        hm.swap_rect(
            snap.rect.x,
            snap.rect.y,
            snap.rect.w,
            snap.rect.h,
            &mut snap.before,
        );
        union = Some(match union {
            Some(u) => u.union(snap.rect),
            None => snap.rect,
        });
    }
    union
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MapSize;

    fn mk_hm() -> Heightmap {
        Heightmap::synth_ramp(MapSize::square(2)) // 129×129
    }

    fn make_snapshot(hm: &Heightmap, x: u32, y: u32, w: u32, h: u32) -> StampSnapshot {
        StampSnapshot {
            rect: DirtyRect { x, y, w, h },
            before: hm.copy_rect(x, y, w, h),
        }
    }

    #[test]
    fn round_trip_single_stamp() {
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();

        // Capture pre-edit values, mutate, store as an undo entry.
        let snap = make_snapshot(&hm, 10, 10, 5, 5);
        for row in 0..5 {
            for col in 0..5 {
                let idx = ((10 + row) * hm.width() + 10 + col) as usize;
                hm.data_mut()[idx] = 0xDEAD;
            }
        }
        let mut h = History::default();
        h.push(UndoEntry { stamps: vec![snap] });

        // Undo restores the original pixels.
        let rect = h.apply_undo(&mut hm).unwrap();
        assert_eq!(
            rect,
            DirtyRect {
                x: 10,
                y: 10,
                w: 5,
                h: 5
            }
        );
        assert_eq!(hm.data(), &orig[..]);
        assert!(h.can_redo() && !h.can_undo());

        // Redo restores the edit.
        let rect2 = h.apply_redo(&mut hm).unwrap();
        assert_eq!(
            rect2,
            DirtyRect {
                x: 10,
                y: 10,
                w: 5,
                h: 5
            }
        );
        let idx = (10 * hm.width() + 10) as usize;
        assert_eq!(hm.data()[idx], 0xDEAD);
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn overlapping_stamps_in_one_stroke_revert_to_pre_stroke_state() {
        // First stamp writes 0xAAAA. Second stamp (partially overlapping)
        // writes 0xBBBB. After undo we expect the *pre-stroke* values, not
        // the intermediate 0xAAAA state.
        let mut hm = mk_hm();
        let orig = hm.data().to_vec();

        let mut entry = UndoEntry::new();

        // Stamp A: 4x4 at (10,10)
        entry.push(make_snapshot(&hm, 10, 10, 4, 4));
        for row in 0..4 {
            for col in 0..4 {
                let idx = ((10 + row) * hm.width() + 10 + col) as usize;
                hm.data_mut()[idx] = 0xAAAA;
            }
        }
        // Stamp B: 4x4 at (12,12) — overlaps A in a 2x2 region
        entry.push(make_snapshot(&hm, 12, 12, 4, 4));
        for row in 0..4 {
            for col in 0..4 {
                let idx = ((12 + row) * hm.width() + 12 + col) as usize;
                hm.data_mut()[idx] = 0xBBBB;
            }
        }

        let mut h = History::default();
        h.push(entry);
        h.apply_undo(&mut hm).unwrap();
        assert_eq!(hm.data(), &orig[..], "pre-stroke state must be restored");
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut hm = mk_hm();
        let mut h = History::default();
        h.push(UndoEntry {
            stamps: vec![make_snapshot(&hm, 0, 0, 2, 2)],
        });
        h.apply_undo(&mut hm);
        assert!(h.can_redo());
        h.push(UndoEntry {
            stamps: vec![make_snapshot(&hm, 5, 5, 2, 2)],
        });
        assert!(!h.can_redo(), "new push must clear redo stack");
    }

    #[test]
    fn barrier_clears_everything() {
        let mut hm = mk_hm();
        let mut h = History::default();
        h.push(UndoEntry {
            stamps: vec![make_snapshot(&hm, 0, 0, 4, 4)],
        });
        h.apply_undo(&mut hm);
        h.barrier();
        assert!(!h.can_undo());
        assert!(!h.can_redo());
        assert_eq!(h.bytes(), 0);
    }

    #[test]
    fn cap_evicts_oldest() {
        let mut hm = mk_hm();
        // 16-byte cap: each snapshot is 2*2*2 = 8 bytes; pushing 3 should
        // evict the first.
        let mut h = History::new(16);
        for i in 0..3 {
            h.push(UndoEntry {
                stamps: vec![make_snapshot(&hm, i, i, 2, 2)],
            });
        }
        assert_eq!(h.undo_depth(), 2);
        assert!(h.bytes() <= 16);
        // Undo to drain — apply_undo runs in reverse order on the surviving
        // entries (i=2 then i=1). Run it to confirm no panics.
        while h.apply_undo(&mut hm).is_some() {}
    }

    #[test]
    fn empty_entry_is_not_pushed() {
        let mut h = History::default();
        h.push(UndoEntry::new());
        assert!(!h.can_undo());
    }
}
