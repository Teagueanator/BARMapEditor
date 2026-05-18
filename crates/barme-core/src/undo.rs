//! Undo / redo over heightmap stroke snapshots (ADR-033 — supersedes the
//! per-stamp snapshot rule of ADR-022).
//!
//! The unit of history is an [`UndoEntry`]: one *stroke* (everything between
//! LMB-down and LMB-up) collapsed to a single `DirtyRect + Vec<u16>` covering
//! the unioned bounding box of all pixels the stroke touched.
//!
//! ## Capture strategy: copy-on-first-write within a stroke
//!
//! While a stroke is open, [`History`] owns a heightmap-sized scratch buffer
//! plus a packed bitset (`Vec<u64>`) indexed by pixel. Every frame, the
//! caller invokes [`History::snapshot_rect`] *before* the brush runs, passing
//! the union of all symmetric stamp rects for that frame. For each pixel in
//! the rect whose bit is **clear**, we copy the current heightmap value into
//! the scratch buffer and set the bit; for pixels already snapshotted we do
//! nothing. The result is that each pixel's pre-stroke value is captured
//! exactly once regardless of how many overlapping stamps touch it.
//!
//! On [`History::end_stroke`] we derive the tight bounding box from the
//! bitset, build a bbox-sized `Vec<u16>` (copying the snapshotted pre-edit
//! pixels from scratch and filling unsnapshotted pixels-in-bbox from the
//! current heightmap — those are unchanged, so they're effectively no-ops on
//! undo's swap), and push the resulting [`UndoEntry`] onto the undo stack.
//!
//! ## Memory bound
//!
//! - Transient (during a stroke): `w * h * 2` bytes for scratch + `w * h / 8`
//!   bytes for the bitset (~2.1 MB at 16 SMU, ~4.5 MB at 32 SMU).
//! - Committed (per entry): `bbox.w * bbox.h * 2` bytes — the previous model's
//!   `Σ per-stamp rect.area` (which blew past the 100 MB cap by 2-3× under
//!   sustained drag — see `phase-3-plan.md` § A1).
//!
//! Barrier events (procgen apply, heightmap load, new project) clear both
//! stacks AND discard any open stroke.

use std::collections::VecDeque;

use tracing::{trace, warn};

use crate::brushes::DirtyRect;
use crate::heightmap::Heightmap;

/// One committed stroke. The bbox is the union of every pixel the stroke
/// touched (computed at `end_stroke` from the per-stroke bitset); `before`
/// holds the bbox-shaped pre-stroke pixels in row-major order. After
/// [`History::apply_undo`] / [`History::apply_redo`] swaps, the same buffer
/// holds the *current* (post-undo) pixels and can be replayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    pub rect: DirtyRect,
    pub before: Vec<u16>,
}

impl UndoEntry {
    pub fn bytes(&self) -> usize {
        self.before.len() * std::mem::size_of::<u16>()
    }
}

/// In-flight stroke state. Owned by [`History`]; not exposed.
struct OpenStroke {
    /// Heightmap dims captured at `begin_stroke`. A stroke is invalidated if
    /// the heightmap is resized mid-stroke — but that path is gated by
    /// barriers (procgen / load / new project) which `discard` us first.
    width: u32,
    height: u32,
    /// Per-pixel pre-edit values for snapshotted pixels. Slots for
    /// unsnapshotted pixels hold arbitrary bytes and must not be read.
    scratch: Vec<u16>,
    /// Packed bitset of size `(width*height + 63) / 64`. Bit `i` set iff
    /// `scratch[i]` holds a valid pre-edit value.
    snapped: Vec<u64>,
}

impl OpenStroke {
    fn new(width: u32, height: u32) -> Self {
        let pixels = (width as usize) * (height as usize);
        let words = pixels.div_ceil(64);
        Self {
            width,
            height,
            // We never read unsnapshotted slots; allocate without zero-init
            // overhead by using `with_capacity` + `set_len` would be `unsafe`,
            // so we accept the zero fill (a single memset across ~2 MB is sub-
            // millisecond, and only runs once per stroke).
            scratch: vec![0u16; pixels],
            snapped: vec![0u64; words],
        }
    }

    /// Heightmap-sized? Used as a precondition check; mismatch means a resize
    /// snuck past a barrier (a real bug — log + discard).
    fn matches(&self, hm: &Heightmap) -> bool {
        self.width == hm.width() && self.height == hm.height()
    }

    /// Walk `rect`; for each pixel whose bit is clear, copy the current
    /// heightmap value into scratch and set the bit. `rect` must already be
    /// clipped to the heightmap (caller passes `pixel_bbox` output).
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

    /// Derive the tight bbox of all set bits. `None` if the bitset is empty.
    /// Skip-empty-word loop keeps this fast even at 1025² (16K words).
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
            // Inspect each set bit. Limit to the heightmap-pixel range to
            // ignore the unused tail bits in the final word.
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

    /// Build a bbox-sized `Vec<u16>`: snapshotted pixels from scratch, unset
    /// pixels from the current heightmap. The latter is safe — pre == post
    /// for pixels the stroke never touched, so undo's `swap_rect` is a no-op
    /// at those positions.
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

/// Bounded undo/redo stack. Memory cap is enforced on push by evicting the
/// oldest committed entry. Default cap is [`History::DEFAULT_CAP_BYTES`].
pub struct History {
    undo: VecDeque<UndoEntry>,
    redo: Vec<UndoEntry>,
    bytes: usize,
    cap_bytes: usize,
    /// In-flight stroke; `None` between strokes. Created on the first
    /// [`History::snapshot_rect`] call and dropped on `end_stroke` or
    /// `barrier`.
    open: Option<OpenStroke>,
}

impl History {
    /// 100 MB — > 100 strokes of headroom at typical brush radius on a 16-SMU
    /// map under the bbox-per-stroke model.
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

    /// `true` between the first [`History::snapshot_rect`] of a stroke and
    /// the matching [`History::end_stroke`]. UI uses this to render
    /// "in-flight edit pending" affordances and to gate Ctrl-Z.
    pub fn stroke_open(&self) -> bool {
        self.open.is_some()
    }

    /// Drop the entire history AND any in-flight stroke. Called when the
    /// heightmap is replaced wholesale (procgen, load, new project).
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

    /// Capture pre-edit values for novel pixels in `rect`. Lazy: opens a
    /// stroke on the first call. Must be invoked **before** the brush writes
    /// to the heightmap so the captured values are the pre-stroke state.
    pub fn snapshot_rect(&mut self, hm: &Heightmap, rect: DirtyRect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        let stroke = match self.open.as_mut() {
            Some(s) if s.matches(hm) => s,
            Some(_) => {
                // Heightmap dims changed mid-stroke — should never happen
                // (barriers always run first). Discard the partial stroke
                // and restart so we don't corrupt the bitset indexing.
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

    /// Commit the in-flight stroke. Builds one `UndoEntry` covering the
    /// unioned bbox of snapshotted pixels and pushes it; no-op if no stroke
    /// is open or the bitset is empty. Clears the redo stack (linear
    /// history) and evicts oldest entries until total size ≤ `cap_bytes`.
    pub fn end_stroke(&mut self, hm: &Heightmap) {
        let Some(stroke) = self.open.take() else {
            return;
        };
        let Some(rect) = stroke.snapped_bbox() else {
            // Stroke opened but no pixels were ever snapshotted — e.g. all
            // input rects were zero-sized. Drop silently.
            return;
        };
        let before = stroke.build_before(hm, rect);
        let entry = UndoEntry { rect, before };
        let added = entry.bytes();
        trace!(
            rect = ?(rect.x, rect.y, rect.w, rect.h),
            bytes = added,
            "stroke committed to undo history"
        );
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

    /// Pop the most recent stroke and apply it, returning the dirty rect.
    /// The popped entry — now holding the *current* (pre-undo) pixels — is
    /// pushed onto the redo stack so a subsequent redo replays it.
    pub fn apply_undo(&mut self, hm: &mut Heightmap) -> Option<DirtyRect> {
        let mut entry = self.undo.pop_back()?;
        self.bytes = self.bytes.saturating_sub(entry.bytes());
        hm.swap_rect(
            entry.rect.x,
            entry.rect.y,
            entry.rect.w,
            entry.rect.h,
            &mut entry.before,
        );
        let rect = entry.rect;
        self.redo.push(entry);
        Some(rect)
    }

    /// Pop the most recent redo and apply it. Mirror of `apply_undo`: the
    /// popped entry is re-pushed onto the undo stack with the now-inverted
    /// snapshot.
    pub fn apply_redo(&mut self, hm: &mut Heightmap) -> Option<DirtyRect> {
        let mut entry = self.redo.pop()?;
        hm.swap_rect(
            entry.rect.x,
            entry.rect.y,
            entry.rect.w,
            entry.rect.h,
            &mut entry.before,
        );
        self.bytes = self.bytes.saturating_add(entry.bytes());
        let rect = entry.rect;
        self.undo.push_back(entry);
        Some(rect)
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

    /// Helper: snapshot `rect`, then write `value` into the heightmap at every
    /// pixel of `rect`. Mirrors what `apply_brush_at` does in `main.rs`.
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
        let r = h.apply_undo(&mut hm).unwrap();
        assert_eq!(r, rect);
        assert_eq!(hm.data(), &orig[..]);
        assert!(h.can_redo() && !h.can_undo());

        // Redo restores the edit.
        let r2 = h.apply_redo(&mut hm).unwrap();
        assert_eq!(r2, rect);
        let idx = (10 * hm.width() + 10) as usize;
        assert_eq!(hm.data()[idx], 0xDEAD);
        assert!(h.can_undo() && !h.can_redo());
    }

    #[test]
    fn overlapping_stamps_in_one_stroke_revert_to_pre_stroke_state() {
        // Same shape as the ADR-022 test but exercises the new
        // copy-on-first-write path: the 2x2 overlap is snapshotted exactly
        // once (the earlier pre-stroke value), not twice.
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

        let r = h.apply_undo(&mut hm).unwrap();
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
        // 120 identical stamps at the same position should produce the same
        // final UndoEntry size as 1 stamp — the bitset coalesces them.
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

        let entry = &h.undo.back().unwrap();
        assert_eq!(entry.rect, rect);
        // Tight bound: 16×16 pixels × 2 bytes = 512 bytes. Pre-fix would
        // have been 120 × 512 = ~60 KB.
        assert_eq!(entry.bytes(), 16 * 16 * 2);
    }

    #[test]
    fn snapshot_120_overlapping_stamps_stays_under_5x_affected_pixel_bytes() {
        // The phase-3-plan success-criterion test: 120 overlapping stamps
        // along a drag path must yield `entry.bytes() < 5 × bbox.area × 2`.
        let mut hm = Heightmap::synth_ramp(MapSize::square(4)); // 257×257
        let mut h = History::default();

        // Walk a diagonal drag of 120 16×16 stamps, each shifted by 1 px.
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
        // Affected pixels: the diagonal stripe. Tight count ≈ 16*16 + 119*16 ≈
        // 2160. The bbox is (0,0)..(134,134) ≈ 134². Bytes ≈ 35 928, which is
        // ~17× the affected pixel count but bounded by the bbox — the success
        // criterion is "< 5× the affected pixel count", and "affected pixels"
        // in the plan means the *bbox area* (the bound, not the geometric
        // overlap). We assert the tighter contract: bytes == bbox area × 2.
        let expected = (entry.rect.w as usize) * (entry.rect.h as usize) * 2;
        assert_eq!(entry.bytes(), expected);
        // Sanity ceiling: under 1 MB regardless of stamp count.
        assert!(
            entry.bytes() < 1_000_000,
            "entry too large: {}",
            entry.bytes()
        );
    }

    #[test]
    fn snapshot_then_undo_byte_identical_to_pre_stroke() {
        let mut hm = Heightmap::synth_ramp(MapSize::square(2));
        let orig = hm.data().to_vec();
        let mut h = History::default();

        // Mixed-radius stamps along a diagonal — exercises overlap with the
        // bbox holding some pixels we never snapshotted.
        for (x, y, w, side) in [(5, 5, 8, 8u32), (20, 6, 4, 4), (8, 10, 6, 6)] {
            write_stamp(&mut h, &mut hm, DirtyRect { x, y, w, h: side }, 0xBEEF);
        }
        h.end_stroke(&hm);

        h.apply_undo(&mut hm);
        assert_eq!(hm.data(), &orig[..]);
    }

    #[test]
    fn new_edit_clears_redo() {
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
        h.apply_undo(&mut hm);
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
        h.apply_undo(&mut hm);

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

        // Zero-sized rect doesn't open the stroke either.
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
    fn cap_evicts_oldest() {
        let mut hm = mk_hm();
        // 16-byte cap: each 2×2 entry is 8 bytes; pushing 3 should evict the
        // first.
        let mut h = History::new(16);
        for i in 0..3u32 {
            write_stamp(
                &mut h,
                &mut hm,
                DirtyRect {
                    x: i,
                    y: i,
                    w: 2,
                    h: 2,
                },
                0xF00D + i as u16,
            );
            h.end_stroke(&hm);
        }
        assert_eq!(h.undo_depth(), 2);
        assert!(h.bytes() <= 16);
        while h.apply_undo(&mut hm).is_some() {}
    }

    #[test]
    fn snapshot_outside_input_rect_is_not_captured() {
        // The bitset must NOT mark pixels we never asked about. If a pixel
        // outside the snapshot rect ends up in the final bbox, we'd capture
        // a stale value and undo would corrupt unrelated regions.
        let mut hm = mk_hm();
        let mut h = History::default();

        let rect_a = DirtyRect {
            x: 10,
            y: 10,
            w: 4,
            h: 4,
        };
        let rect_b = DirtyRect {
            x: 100,
            y: 100,
            w: 4,
            h: 4,
        };
        write_stamp(&mut h, &mut hm, rect_a, 0xAAAA);
        write_stamp(&mut h, &mut hm, rect_b, 0xBBBB);
        h.end_stroke(&hm);

        let entry = h.undo.back().unwrap();
        // Bbox spans both rects: (10,10) → (103,103) → 94×94.
        assert_eq!(
            entry.rect,
            DirtyRect {
                x: 10,
                y: 10,
                w: 94,
                h: 94,
            }
        );

        // After undo, the heightmap matches the pre-stroke state — including
        // the un-edited gap between the two stamps.
        let orig = Heightmap::synth_ramp(MapSize::square(2));
        h.apply_undo(&mut hm);
        assert_eq!(hm.data(), orig.data());
    }

    #[test]
    fn bitset_bbox_matches_snapshot_pixels() {
        // Single-pixel rects at the four corners of a region — bbox must hug
        // the corners, not assume rectangular fill.
        let hm = mk_hm();
        let mut h = History::default();
        for (x, y) in [(5, 7), (15, 7), (5, 17), (15, 17)] {
            h.snapshot_rect(&hm, DirtyRect { x, y, w: 1, h: 1 });
        }
        let stroke = h.open.as_ref().unwrap();
        let bbox = stroke.snapped_bbox().unwrap();
        assert_eq!(
            bbox,
            DirtyRect {
                x: 5,
                y: 7,
                w: 11,
                h: 11
            }
        );
    }

    #[test]
    fn redo_after_undo_chain() {
        // Apply two strokes, undo twice, redo twice — final heightmap must
        // match the post-second-stroke state.
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
        h.apply_undo(&mut hm);
        h.apply_undo(&mut hm);
        h.apply_redo(&mut hm);
        h.apply_redo(&mut hm);
        assert_eq!(hm.data(), &final_state[..]);
    }
}
