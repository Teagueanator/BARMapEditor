//! Tiled copy-on-write mask storage (D9 / Sprint 16, ADR-039).
//!
//! Sprint 15 ([`super`] ADR-038) shipped [`LayerMask`] backed by a flat
//! `Vec<u8>` — convenient, but 64 MB resident *per layer* on a 16-SMU
//! map regardless of how much of the layer is actually painted. With a
//! 16-layer cap that lands at 1 GB before the user touches a brush.
//!
//! Sprint 16 swaps the storage for a grid of 256² `Tile`s where each
//! tile is either:
//!
//! - [`Tile::Uniform`] — every pixel in the tile is the same byte
//!   (default for fresh layers; a bottom-base layer is one fill across
//!   the whole grid). Cost: a single byte payload + the 16-byte enum
//!   tag.
//! - [`Tile::Pixels`] — a heap-allocated `Box<[u8; 256 × 256]>`. Cost:
//!   64 KB per concrete tile. Allocated lazily on the first write that
//!   touches a `Uniform` tile via [`promote_to_pixels`].
//!
//! Memory scales with paint coverage, NOT map size. A typical brush
//! stroke touches ~5–20 tiles (~320 KB – 1.3 MB allocated); an
//! unpainted layer fits in ~16 KB regardless of map size.
//!
//! The public [`LayerMask`] surface is preserved from Sprint 15
//! ([`Self::filled`] / [`Self::sample`] / [`Self::write_rect`]) so the
//! D8 bake path keeps compiling. Sprint 16's brushes additionally call
//! [`LayerMask::write_rect_with`] (functional per-pixel writes, no
//! intermediate buffer) and [`LayerMask::dirty_tiles_since`] (which
//! tiles changed since a given version cursor — drives the
//! [`super::super`] ADR-039 GPU dirty-tile upload).
//!
//! ## Sprint-15 → Sprint-16 migration
//!
//! Existing `.barmeproj` files store `LayerMask` as `{ width, height,
//! bytes: <base64> }`. The new wire shape is `{ width, height, tiles:
//! Vec<String> }` — see [`tile_wire`] for the per-tile encoding. The
//! custom [`Deserialize`] impl accepts either; the legacy flat-bytes
//! path runs [`TileGrid::from_flat_bytes`] which detects uniform
//! tiles and collapses them, so a freshly-loaded Sprint-15 project
//! gets the same memory profile as a freshly-created one.
//! Pinned by [`tests::legacy_flat_bytes_round_trip_compresses_uniform_tiles`].

use std::collections::HashSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::MapSize;
use crate::brushes::DirtyRect;

/// Side of a single tile in pixels. Picked at 256 so 4 tiles cover
/// 1 SMU's worth of mask (`SMU = 2 → 1024² → 4×4 tiles`,
/// `SMU = 16 → 8192² → 32×32 tiles`). Smaller would balloon the
/// per-tile enum-tag overhead; larger would defeat the "small stroke
/// allocates one tile" win.
pub const TILE_DIM: u32 = 256;

/// `TILE_DIM × TILE_DIM` = 65 536 bytes per concrete tile.
pub const TILE_PIXELS: usize = (TILE_DIM as usize) * (TILE_DIM as usize);

/// One cell in the [`TileGrid`].
///
/// **Sprint 17 (ADR-041):** promoted from module-private to `pub` so
/// per-stroke mask undo (see [`crate::undo::MaskEntry`]) can hold
/// before/after snapshots without re-exporting the enum behind a
/// newtype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tile {
    /// All `TILE_PIXELS` pixels are this byte. Allocates 0 bytes on
    /// the heap.
    Uniform(u8),
    /// Concrete row-major pixels. The `Box` keeps the enum size small
    /// (8 B pointer + 1 B discriminant; padded to 16 B per Tile) so
    /// the surrounding `Vec<Tile>` stays compact for sparse grids.
    Pixels(Box<[u8; TILE_PIXELS]>),
}

impl Tile {
    /// Resident-byte cost of one tile snapshot. Used by
    /// [`crate::undo::MaskEntry::bytes`] to keep the 100 MB cap honest.
    pub fn resident_bytes(&self) -> usize {
        match self {
            Tile::Uniform(_) => std::mem::size_of::<Tile>(),
            Tile::Pixels(_) => TILE_PIXELS + std::mem::size_of::<Tile>(),
        }
    }
}

impl Tile {
    /// Read `local` (in `0..TILE_DIM`²) from the tile.
    fn read(&self, local_x: u32, local_y: u32) -> u8 {
        match self {
            Tile::Uniform(b) => *b,
            Tile::Pixels(p) => p[(local_y * TILE_DIM + local_x) as usize],
        }
    }
}

/// Promote a `Uniform`-tagged tile in `grid` to a freshly-allocated
/// `Pixels` tile pre-filled with the uniform byte. No-op if the tile
/// is already `Pixels`. Returns a mutable reference to the (now
/// concrete) pixel buffer.
fn promote_to_pixels(tile: &mut Tile) -> &mut [u8; TILE_PIXELS] {
    if let Tile::Uniform(b) = *tile {
        // Box::new on a 64 KB array would blow the stack on debug
        // builds; vec![; N].into_boxed_slice() heap-allocates directly.
        let v = vec![b; TILE_PIXELS].into_boxed_slice();
        let arr: Box<[u8; TILE_PIXELS]> = v
            .try_into()
            .expect("vec of TILE_PIXELS bytes converts to Box<[u8; TILE_PIXELS]>");
        *tile = Tile::Pixels(arr);
    }
    match tile {
        Tile::Pixels(p) => p,
        Tile::Uniform(_) => unreachable!("promoted above"),
    }
}

/// Tile coordinate (in tile, not pixel, units). `dirty_tiles_since`
/// returns these so the GPU upload knows which tiles to push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCoord {
    pub tile_x: u32,
    pub tile_y: u32,
}

/// Internal storage for [`LayerMask`]. Public only within the layers
/// module; the outside world goes through [`LayerMask`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct TileGrid {
    width: u32,
    height: u32,
    tiles_x: u32,
    tiles_y: u32,
    tiles: Vec<Tile>,
    /// Monotonic write version. Bumped once per `write_*` invocation
    /// regardless of how many tiles were touched. Callers (ADR-039
    /// GPU compositor) capture the latest value after each upload
    /// pass and pass it back to [`Self::dirty_tiles_since`] next frame.
    current_version: u64,
    /// `tile_versions[idx]` == the `current_version` value at the
    /// most recent write that touched tile `idx`. Initially 0 so a
    /// caller passing `since = 0` gets every tile back on the first
    /// scan.
    tile_versions: Vec<u64>,
}

impl TileGrid {
    fn filled(width: u32, height: u32, fill: u8) -> Self {
        let tiles_x = width.div_ceil(TILE_DIM);
        let tiles_y = height.div_ceil(TILE_DIM);
        let tile_count = (tiles_x as usize) * (tiles_y as usize);
        // Sprint 23 (T1 / H2): the initial version depends on `fill`.
        // wgpu zero-initialises every texture on allocation, so when
        // the CPU mask is also uniform-zero the GPU side already
        // matches — no cold-sync upload required. For non-zero fills
        // (the bottom-base layer convention is `fill = 255`) the
        // GPU's zero default does NOT match, so every tile must
        // upload on the first sync.
        //
        // Pre-Sprint-23 this was unconditionally `1`, which fanned
        // 1024 × layer_count cold-sync uploads on 16-SMU projects
        // (~256 MB transferred per entry on a 4-layer stack —
        // dominant contributor to the PaintLayer-entry OOM).
        let initial_version: u64 = if fill == 0 { 0 } else { 1 };
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            tiles: vec![Tile::Uniform(fill); tile_count],
            current_version: initial_version,
            tile_versions: vec![initial_version; tile_count],
        }
    }

    /// Build a tile grid from a flat row-major `Vec<u8>`. Used by the
    /// Sprint-15 wire-format migration: scans each tile for runs of
    /// identical bytes and collapses to `Uniform` where possible.
    /// A freshly-flushed full-fill mask compresses to all-`Uniform`,
    /// matching the [`Self::filled`] output exactly.
    fn from_flat_bytes(width: u32, height: u32, bytes: &[u8]) -> Self {
        let expected = (width as usize) * (height as usize);
        // Defensive: a malformed file shouldn't blow up the loader.
        // Truncate / pad as needed; the loaded mask may be wrong but
        // the editor still opens for the user to repair.
        let bytes = if bytes.len() == expected {
            std::borrow::Cow::Borrowed(bytes)
        } else {
            let mut padded = vec![0u8; expected];
            let n = bytes.len().min(expected);
            padded[..n].copy_from_slice(&bytes[..n]);
            std::borrow::Cow::Owned(padded)
        };
        let tiles_x = width.div_ceil(TILE_DIM);
        let tiles_y = height.div_ceil(TILE_DIM);
        let mut tiles = Vec::with_capacity((tiles_x as usize) * (tiles_y as usize));
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                tiles.push(build_tile_from_flat(&bytes, width, height, tx, ty));
            }
        }
        let tile_count = tiles.len();
        Self {
            width,
            height,
            tiles_x,
            tiles_y,
            tiles,
            current_version: 1,
            tile_versions: vec![1; tile_count],
        }
    }

    fn tile_index(&self, tx: u32, ty: u32) -> usize {
        (ty * self.tiles_x + tx) as usize
    }
}

/// Extract one tile's worth of pixels from a flat byte slice. Tiles on
/// the right / bottom map edges that the diffuse dim doesn't fully
/// cover get padded with their first-pixel value — the result still
/// reads back correctly because [`LayerMask::sample`] clips to
/// `width × height` before any tile lookup.
fn build_tile_from_flat(bytes: &[u8], width: u32, height: u32, tx: u32, ty: u32) -> Tile {
    let x0 = tx * TILE_DIM;
    let y0 = ty * TILE_DIM;
    // First pixel of the tile drives the Uniform-detection candidate.
    // Sample inside `width × height` even on edge tiles (clip).
    let cx = x0.min(width.saturating_sub(1));
    let cy = y0.min(height.saturating_sub(1));
    let candidate = bytes[(cy as usize) * (width as usize) + (cx as usize)];

    let mut uniform = true;
    let x_end = (x0 + TILE_DIM).min(width);
    let y_end = (y0 + TILE_DIM).min(height);
    'scan: for y in y0..y_end {
        for x in x0..x_end {
            if bytes[(y as usize) * (width as usize) + (x as usize)] != candidate {
                uniform = false;
                break 'scan;
            }
        }
    }
    if uniform {
        return Tile::Uniform(candidate);
    }

    // Concrete tile — copy into a freshly-allocated 256² array. Edge
    // pixels (where x_end < TILE_DIM or y_end < TILE_DIM) get padded
    // with `candidate` so reads inside `width × height` are exact and
    // out-of-range reads (which `sample` rejects up front) are deterministic.
    let mut buf = vec![candidate; TILE_PIXELS].into_boxed_slice();
    for y in y0..y_end {
        let local_y = (y - y0) as usize;
        let src_row = (y as usize) * (width as usize) + (x0 as usize);
        let dst_row = local_y * (TILE_DIM as usize);
        let copy_w = (x_end - x0) as usize;
        buf[dst_row..dst_row + copy_w].copy_from_slice(&bytes[src_row..src_row + copy_w]);
    }
    let arr: Box<[u8; TILE_PIXELS]> = buf
        .try_into()
        .expect("buf len matches TILE_PIXELS by construction");
    Tile::Pixels(arr)
}

/// Grayscale alpha mask sized to the diffuse (`512 × SMU` per side).
///
/// Storage is tiled copy-on-write: 256² byte tiles, each either
/// uniform (`Tile::Uniform(b)`) or concrete (`Tile::Pixels(Box<[u8;
/// 65 536]>)`). Fresh layers cost ~16 KB regardless of map size;
/// painted regions allocate concrete tiles on first write.
///
/// `bytes[i] = 255` → layer fully visible at pixel i.
/// `bytes[i] = 0`   → fully transparent (lower layers show through).
///
/// **Memory.** A 16-SMU map mask has 32 × 32 = 1 024 tiles; an
/// empty layer = 16 KB, a fully-painted layer = 64 MB (matches the
/// pre-Sprint-16 cost), a typical brush stroke touching ~5–20 tiles
/// allocates ~320 KB – 1.3 MB. Memory scales with paint coverage,
/// not map size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerMask {
    pub width: u32,
    pub height: u32,
    grid: TileGrid,
}

impl LayerMask {
    /// Allocate a mask filled with `fill` at `size.texture_dims()`.
    /// Sprint 16: backed by a `Vec<Tile::Uniform(fill)>` — ~16 KB at
    /// 16 SMU regardless of `fill`.
    pub fn filled(size: MapSize, fill: u8) -> Self {
        let (w, h) = size.texture_dims();
        Self {
            width: w,
            height: h,
            grid: TileGrid::filled(w, h, fill),
        }
    }

    /// Sample the mask at integer pixel coordinates. Returns `0` for
    /// out-of-bounds reads (defensive — the compositor clips at the
    /// call site).
    ///
    /// `Uniform` tiles hit the fast path: one bounds check + one
    /// `match` arm + one byte return, no heap dereference. The bake
    /// path in [`super::LayerStack::bake_diffuse`] reads ~256
    /// megapixels per 16-SMU bake, so the fast path matters.
    pub fn sample(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        let tx = x / TILE_DIM;
        let ty = y / TILE_DIM;
        let lx = x % TILE_DIM;
        let ly = y % TILE_DIM;
        let idx = self.grid.tile_index(tx, ty);
        self.grid.tiles[idx].read(lx, ly)
    }

    /// Resident-byte cost — the live size of the tile grid plus all
    /// allocated `Pixels` tiles. Drives the [`crate::undo`] cap
    /// accounting + the GPU compositor's heuristic for "this layer
    /// has grown beyond the preview budget".
    ///
    /// Replaces Sprint-15's `bytes.capacity()` accessor; pre-Sprint-16
    /// callers should switch to this.
    pub fn resident_bytes(&self) -> usize {
        let pixels: usize = self
            .grid
            .tiles
            .iter()
            .filter(|t| matches!(t, Tile::Pixels(_)))
            .count()
            * TILE_PIXELS;
        // Tile enum is 16 bytes on 64-bit targets (1-byte payload or
        // 8-byte Box pointer, plus discriminant, plus alignment).
        // `Vec<u64>` stride for the version array is 8 bytes per tile.
        let inline = self.grid.tiles.len() * (std::mem::size_of::<Tile>() + 8);
        inline + pixels
    }

    /// Write a rect of bytes into the mask. Out-of-bounds writes
    /// silently clip; returns the clipped `(x, y, w, h)` actually
    /// written. Kept for the Sprint-15 ABI (the D8 bake path doesn't
    /// call it, but the field's still in the surface area).
    ///
    /// Touched tiles promote from `Uniform` to `Pixels` lazily.
    pub fn write_rect(
        &mut self,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        src: &[u8],
    ) -> Option<(u32, u32, u32, u32)> {
        if w == 0 || h == 0 || x >= self.width || y >= self.height {
            return None;
        }
        let cw = w.min(self.width - x);
        let ch = h.min(self.height - y);
        self.write_rect_with(x, y, cw, h.min(self.height - y), |dx, dy| {
            let row = dy as usize;
            let col = dx as usize;
            // `src` is `w` wide (the original, un-clipped width). Reads
            // past `cw` would still be inside the caller-supplied slice
            // by definition.
            src[row * (w as usize) + col]
        });
        Some((x, y, cw, ch))
    }

    /// Functional per-pixel write across the clipped rect. The brush
    /// kernels in [`super::brushes`] use this to avoid allocating an
    /// intermediate `Vec<u8>` per stamp.
    ///
    /// `f` receives `(local_x, local_y)` in `0..rect.w × 0..rect.h`
    /// (NOT absolute map coords). The returned byte is what lands at
    /// `(rect.x + local_x, rect.y + local_y)` in the mask.
    ///
    /// Tiles outside the clipped rect are untouched. Tiles inside that
    /// are wholly covered by `f` returning the same byte for every
    /// pixel COULD be collapsed back to `Uniform`, but the cost of
    /// detecting that is `TILE_PIXELS` reads per tile per stamp — not
    /// worth it for the typical sub-tile brush. The mask DOES stay
    /// honest because `write_rect_with` always bumps `current_version`
    /// and per-tile versions, so the dirty-upload path is correct.
    pub fn write_rect_with<F: FnMut(u32, u32) -> u8>(
        &mut self,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        mut f: F,
    ) -> Option<DirtyRect> {
        if w == 0 || h == 0 || x >= self.width || y >= self.height {
            return None;
        }
        let cw = w.min(self.width - x);
        let ch = h.min(self.height - y);
        if cw == 0 || ch == 0 {
            return None;
        }
        // Bump the global write version once per stamp so all tiles
        // touched by this stroke share a single version tag.
        self.grid.current_version += 1;
        let new_version = self.grid.current_version;

        let tx0 = x / TILE_DIM;
        let ty0 = y / TILE_DIM;
        let tx1 = (x + cw - 1) / TILE_DIM;
        let ty1 = (y + ch - 1) / TILE_DIM;

        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let tile_x0 = tx * TILE_DIM;
                let tile_y0 = ty * TILE_DIM;
                let x_lo = x.max(tile_x0);
                let y_lo = y.max(tile_y0);
                let x_hi = (x + cw).min(tile_x0 + TILE_DIM);
                let y_hi = (y + ch).min(tile_y0 + TILE_DIM);
                let idx = self.grid.tile_index(tx, ty);
                let pixels = promote_to_pixels(&mut self.grid.tiles[idx]);
                for yy in y_lo..y_hi {
                    let local_y = yy - tile_y0;
                    let dy = yy - y;
                    let row_base = (local_y * TILE_DIM) as usize;
                    for xx in x_lo..x_hi {
                        let local_x = xx - tile_x0;
                        let dx = xx - x;
                        pixels[row_base + local_x as usize] = f(dx, dy);
                    }
                }
                self.grid.tile_versions[idx] = new_version;
            }
        }
        Some(DirtyRect { x, y, w: cw, h: ch })
    }

    /// Direct single-pixel write. Promotes the containing tile if
    /// needed. Bumps the version. Intended for [`super::brushes`]'
    /// flood-fill brush which writes sparsely along an irregular
    /// visited set; the per-pixel call is much cheaper than building a
    /// dense buffer first.
    pub fn set_pixel(&mut self, x: u32, y: u32, value: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        self.grid.current_version += 1;
        let tx = x / TILE_DIM;
        let ty = y / TILE_DIM;
        let lx = x % TILE_DIM;
        let ly = y % TILE_DIM;
        let idx = self.grid.tile_index(tx, ty);
        let pixels = promote_to_pixels(&mut self.grid.tiles[idx]);
        pixels[(ly * TILE_DIM + lx) as usize] = value;
        self.grid.tile_versions[idx] = self.grid.current_version;
    }

    /// Latest write version (monotonic). The GPU compositor captures
    /// this after each upload pass; the next pass passes it back to
    /// [`Self::dirty_tiles_since`].
    pub fn version(&self) -> u64 {
        self.grid.current_version
    }

    /// Tile coordinates whose pixel data has changed since `since`.
    /// `since = 0` returns every tile (cold-start case).
    pub fn dirty_tiles_since(&self, since: u64) -> Vec<TileCoord> {
        let mut out = Vec::new();
        for (idx, &v) in self.grid.tile_versions.iter().enumerate() {
            if v > since {
                let tx = (idx as u32) % self.grid.tiles_x;
                let ty = (idx as u32) / self.grid.tiles_x;
                out.push(TileCoord {
                    tile_x: tx,
                    tile_y: ty,
                });
            }
        }
        out
    }

    /// Read one tile's worth of pixels into the caller's buffer for
    /// GPU upload. `dst` MUST be `TILE_PIXELS` bytes. Uniform tiles
    /// fill the buffer; concrete tiles `memcpy`.
    ///
    /// Reads cover the full `TILE_DIM²` even when the tile crosses
    /// the right / bottom map edge; the unused trailing bytes match
    /// whatever `from_flat_bytes` / `promote_to_pixels` left there
    /// (the candidate fill byte). The GPU upload path only copies
    /// `min(TILE_DIM, width - tile_x0) × min(TILE_DIM, height -
    /// tile_y0)` of those pixels into the texture.
    pub fn read_tile(&self, tx: u32, ty: u32, dst: &mut [u8; TILE_PIXELS]) {
        let idx = self.grid.tile_index(tx, ty);
        match &self.grid.tiles[idx] {
            Tile::Uniform(b) => dst.fill(*b),
            Tile::Pixels(p) => dst.copy_from_slice(&p[..]),
        }
    }

    /// Per-axis tile counts. Exposed so the GPU compositor can
    /// pre-size its dirty-tile staging buffers.
    pub fn tile_grid_dims(&self) -> (u32, u32) {
        (self.grid.tiles_x, self.grid.tiles_y)
    }

    /// Test-only: count of `Pixels`-variant tiles in the grid. The
    /// Sprint-15 migration tests use this to assert that a flat-bytes
    /// all-uniform input collapses to zero allocated pixel buffers.
    #[cfg(test)]
    pub(crate) fn allocated_tile_count(&self) -> usize {
        self.grid
            .tiles
            .iter()
            .filter(|t| matches!(t, Tile::Pixels(_)))
            .count()
    }

    /// D10 / Sprint 17 (ADR-041) — return a clone of the tile at
    /// `coord`. Used by [`crate::undo::OpenMaskStroke`] to capture
    /// the pre-stroke state of every tile a brush stamp will touch.
    /// `Uniform` tiles clone cheaply (one byte); `Pixels` tiles
    /// allocate a fresh `Box<[u8; TILE_PIXELS]>` — the 64 KB cost is
    /// the per-tile undo cost noted in ADR-041.
    ///
    /// Returns `Tile::Uniform(0)` for out-of-range coords (defensive;
    /// the caller is expected to pass valid coords from
    /// [`Self::tile_coords_overlapping_rect`]).
    pub fn clone_tile(&self, coord: TileCoord) -> Tile {
        if coord.tile_x >= self.grid.tiles_x || coord.tile_y >= self.grid.tiles_y {
            return Tile::Uniform(0);
        }
        let idx = self.grid.tile_index(coord.tile_x, coord.tile_y);
        self.grid.tiles[idx].clone()
    }

    /// D10 / Sprint 17 (ADR-041) — restore a single tile at `coord`
    /// from a snapshot. Bumps the global write version + the per-tile
    /// version so the GPU compositor picks up the change. Used by the
    /// undo dispatcher to reverse a brush stroke.
    ///
    /// Out-of-range coords silently no-op.
    pub fn restore_tile(&mut self, coord: TileCoord, tile: Tile) {
        if coord.tile_x >= self.grid.tiles_x || coord.tile_y >= self.grid.tiles_y {
            return;
        }
        self.grid.current_version += 1;
        let idx = self.grid.tile_index(coord.tile_x, coord.tile_y);
        self.grid.tiles[idx] = tile;
        self.grid.tile_versions[idx] = self.grid.current_version;
    }

    /// D10 / Sprint 17 (ADR-041) — tile coordinates whose tile rect
    /// intersects `rect` (a pixel-space rectangle). Used by the undo
    /// dispatcher's pre-stamp snapshot path: the brush bbox maps to
    /// a small set of tiles, each of which gets cloned into the open
    /// stroke before the brush writes.
    ///
    /// Returns empty when `rect` is degenerate or lies entirely
    /// outside the mask.
    pub fn tile_coords_overlapping_rect(&self, rect: DirtyRect) -> Vec<TileCoord> {
        if rect.w == 0 || rect.h == 0 || rect.x >= self.width || rect.y >= self.height {
            return Vec::new();
        }
        let cw = rect.w.min(self.width - rect.x);
        let ch = rect.h.min(self.height - rect.y);
        if cw == 0 || ch == 0 {
            return Vec::new();
        }
        let tx0 = rect.x / TILE_DIM;
        let ty0 = rect.y / TILE_DIM;
        let tx1 = (rect.x + cw - 1) / TILE_DIM;
        let ty1 = (rect.y + ch - 1) / TILE_DIM;
        let mut out = Vec::with_capacity(((tx1 - tx0 + 1) * (ty1 - ty0 + 1)) as usize);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                out.push(TileCoord {
                    tile_x: tx,
                    tile_y: ty,
                });
            }
        }
        out
    }

    /// D10 / Sprint 17 (ADR-041) — pixel-space bbox of the stamp
    /// clipped to the mask. Promoted from `pub(super)` so the app's
    /// pre-stamp snapshot loop can compute the tile set to capture
    /// without re-implementing the math.
    pub fn brush_bbox(&self, stamp: MaskStamp) -> Option<DirtyRect> {
        mask_pixel_bbox(self, stamp)
    }
}

// ---------------------------------------------------------------------------
// Serde — backwards-compat with Sprint 15's flat-bytes wire format
// ---------------------------------------------------------------------------

/// Wire shape used by [`Serialize`]: `{ width, height, tiles:
/// Vec<String> }` where each tile string is `"u:<byte>"` or
/// `"p:<base64-65536-bytes>"`. The string-encoding sidesteps TOML's
/// awkward array-of-table representation for enum variants.
mod tile_wire {
    use super::{BASE64, Engine, TILE_PIXELS, Tile};

    pub fn encode_tiles(tiles: &[Tile]) -> Vec<String> {
        tiles
            .iter()
            .map(|t| match t {
                Tile::Uniform(b) => format!("u:{b}"),
                Tile::Pixels(p) => format!("p:{}", BASE64.encode(p.as_ref())),
            })
            .collect()
    }

    /// Decode one tile-wire string. Errors carry the offending prefix
    /// so a corrupted file surfaces a meaningful diagnostic rather
    /// than a generic base64 / parse error.
    pub fn decode_tile(s: &str) -> Result<Tile, String> {
        if let Some(rest) = s.strip_prefix("u:") {
            let b: u8 = rest
                .parse()
                .map_err(|e: std::num::ParseIntError| format!("uniform: {e}"))?;
            Ok(Tile::Uniform(b))
        } else if let Some(rest) = s.strip_prefix("p:") {
            let bytes = BASE64
                .decode(rest.as_bytes())
                .map_err(|e| format!("pixels base64: {e}"))?;
            if bytes.len() != TILE_PIXELS {
                return Err(format!(
                    "pixels tile expected {TILE_PIXELS} bytes, got {}",
                    bytes.len()
                ));
            }
            let arr: Box<[u8; TILE_PIXELS]> = bytes
                .into_boxed_slice()
                .try_into()
                .map_err(|_| "pixels tile length didn't fit Box<[u8; TILE_PIXELS]>".to_string())?;
            Ok(Tile::Pixels(arr))
        } else {
            Err(format!("unknown tile prefix: {s:?}"))
        }
    }
}

#[derive(Serialize)]
struct LayerMaskTiledWire<'a> {
    width: u32,
    height: u32,
    tiles: Vec<String>,
    #[serde(skip)]
    _marker: std::marker::PhantomData<&'a ()>,
}

/// Wire-shape disambiguator on load. Sprint-16 files match `Tiled`
/// (carries `tiles`); Sprint-15 files match `Flat` (carries `bytes`).
/// `serde(untagged)` walks variants in declaration order, so `Tiled`
/// is tried first.
#[derive(Deserialize)]
#[serde(untagged)]
enum LayerMaskWire {
    Tiled {
        width: u32,
        height: u32,
        tiles: Vec<String>,
    },
    Flat {
        width: u32,
        height: u32,
        #[serde(with = "flat_bytes_b64")]
        bytes: Vec<u8>,
    },
}

/// Sprint-15 wire-shape helper: `bytes` field round-trips through
/// base64. Kept here (rather than in [`super`]) because the legacy
/// path is the only consumer.
mod flat_bytes_b64 {
    use super::{BASE64, Engine};
    use serde::Deserialize;

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        BASE64
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

impl Serialize for LayerMask {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let wire = LayerMaskTiledWire {
            width: self.width,
            height: self.height,
            tiles: tile_wire::encode_tiles(&self.grid.tiles),
            _marker: std::marker::PhantomData,
        };
        wire.serialize(s)
    }
}

impl<'de> Deserialize<'de> for LayerMask {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = LayerMaskWire::deserialize(d)?;
        match wire {
            LayerMaskWire::Tiled {
                width,
                height,
                tiles,
            } => {
                let tiles_x = width.div_ceil(TILE_DIM);
                let tiles_y = height.div_ceil(TILE_DIM);
                let expected_tiles = (tiles_x as usize) * (tiles_y as usize);
                if tiles.len() != expected_tiles {
                    return Err(serde::de::Error::custom(format!(
                        "tile count {} disagrees with dims {}×{} (expected {})",
                        tiles.len(),
                        width,
                        height,
                        expected_tiles
                    )));
                }
                let decoded: Result<Vec<Tile>, String> =
                    tiles.iter().map(|s| tile_wire::decode_tile(s)).collect();
                let decoded = decoded.map_err(serde::de::Error::custom)?;
                let tile_count = decoded.len();
                Ok(LayerMask {
                    width,
                    height,
                    grid: TileGrid {
                        width,
                        height,
                        tiles_x,
                        tiles_y,
                        tiles: decoded,
                        current_version: 1,
                        tile_versions: vec![1; tile_count],
                    },
                })
            }
            LayerMaskWire::Flat {
                width,
                height,
                bytes,
            } => Ok(LayerMask {
                width,
                height,
                grid: TileGrid::from_flat_bytes(width, height, &bytes),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Mask brush helpers — shared falloff math, exported to `super::brushes`
// ---------------------------------------------------------------------------

/// Pixel bounding box of a stamp clipped to the mask. Same shape as
/// [`crate::splat::splat_pixel_bbox`] but in mask-pixel space (1 px
/// per elmo, since the mask is sized at `texture_dims = 512 × SMU`
/// and the elmo extent is also `512 × SMU`).
pub(super) fn mask_pixel_bbox(mask: &LayerMask, stamp: MaskStamp) -> Option<DirtyRect> {
    if mask.width == 0 || mask.height == 0 {
        return None;
    }
    if stamp.radius <= 0.0 {
        return None;
    }
    let cx = stamp.world_x;
    let cz = stamp.world_z;
    let r = stamp.radius;
    let min_x = (cx - r).floor().max(0.0) as i64;
    let max_x = (cx + r).ceil().min((mask.width - 1) as f32) as i64;
    let min_y = (cz - r).floor().max(0.0) as i64;
    let max_y = (cz + r).ceil().min((mask.height - 1) as f32) as i64;
    if max_x < min_x || max_y < min_y {
        return None;
    }
    Some(DirtyRect {
        x: min_x as u32,
        y: min_y as u32,
        w: (max_x - min_x + 1) as u32,
        h: (max_y - min_y + 1) as u32,
    })
}

/// One stamp of a mask brush stroke. World coords are in elmos (which
/// equal mask-pixel coords thanks to the `512 px / SMU = 512 elmos /
/// SMU` identity).
///
/// `target_visible` is consumed by [`super::brushes::MaskFill`] only;
/// the falloff brushes ignore it.
#[derive(Debug, Clone, Copy)]
pub struct MaskStamp {
    pub world_x: f32,
    pub world_z: f32,
    pub radius: f32,
    /// Strength 0..=1. Brush-specific interpretation.
    pub strength: f32,
    /// Only meaningful for [`super::brushes::MaskFill`].
    pub target_visible: bool,
}

/// 4-connected flood fill from `(seed_x, seed_y)` writing
/// `target_byte` to every visited pixel whose pre-fill value is
/// within `±threshold` of the seed pixel's pre-fill value. Returns
/// the bounding box of visited pixels (clipped to mask bounds), or
/// `None` if the seed is off-map.
///
/// Lives next to [`LayerMask`] (rather than under [`super::brushes`])
/// because `set_pixel` + `sample` are the natural primitives — the
/// brush trait just calls in.
pub(super) fn flood_fill(
    mask: &mut LayerMask,
    seed_x: u32,
    seed_y: u32,
    target_byte: u8,
    threshold: u8,
) -> Option<DirtyRect> {
    if seed_x >= mask.width || seed_y >= mask.height {
        return None;
    }
    let seed_value = mask.sample(seed_x, seed_y);
    // Trivial case: seed is already the target byte and within
    // tolerance of itself. We still walk the flood to honour the
    // user's intent ("fill the visually-connected region with this
    // value") even when target == seed — but bail early when the
    // seed pixel already lands at target_byte AND the surrounding
    // tile is uniform, since there's nothing to dirty.
    let mut visited: HashSet<(u32, u32)> = HashSet::new();
    let mut stack: Vec<(u32, u32)> = Vec::new();
    stack.push((seed_x, seed_y));
    let lo = seed_value.saturating_sub(threshold);
    let hi = seed_value.saturating_add(threshold);
    let mut min_x = seed_x;
    let mut max_x = seed_x;
    let mut min_y = seed_y;
    let mut max_y = seed_y;
    while let Some((x, y)) = stack.pop() {
        if !visited.insert((x, y)) {
            continue;
        }
        let v = mask.sample(x, y);
        if v < lo || v > hi {
            visited.remove(&(x, y));
            continue;
        }
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
        // 4-connected neighbours.
        if x > 0 {
            stack.push((x - 1, y));
        }
        if x + 1 < mask.width {
            stack.push((x + 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if y + 1 < mask.height {
            stack.push((x, y + 1));
        }
    }
    if visited.is_empty() {
        return None;
    }
    // Apply target byte to every visited pixel. Per-pixel writes
    // promote tiles lazily as needed; if every visited pixel lands
    // in the same tile, only that one tile allocates.
    for (x, y) in &visited {
        mask.set_pixel(*x, *y, target_byte);
    }
    Some(DirtyRect {
        x: min_x,
        y: min_y,
        w: max_x - min_x + 1,
        h: max_y - min_y + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_smu() -> MapSize {
        MapSize::square(2)
    }

    /// D10 / Sprint 17 (ADR-041): clone_tile + restore_tile round-trip.
    /// Paint a known pattern into one tile, snapshot it, mutate, then
    /// restore — the mask should match the original byte-for-byte.
    #[test]
    fn clone_and_restore_tile_round_trip() {
        let mut m = LayerMask::filled(two_smu(), 0);
        // Promote (0, 0) tile to Pixels by writing a single pixel.
        m.set_pixel(10, 10, 240);
        let coord = TileCoord {
            tile_x: 0,
            tile_y: 0,
        };
        let snap = m.clone_tile(coord);
        assert!(matches!(snap, Tile::Pixels(_)));
        // Overwrite — paint a giant rect that covers the tile.
        m.write_rect_with(0, 0, 256, 256, |_, _| 5);
        assert_eq!(m.sample(10, 10), 5);
        // Restore.
        m.restore_tile(coord, snap);
        assert_eq!(m.sample(10, 10), 240);
        assert_eq!(m.sample(11, 11), 0);
    }

    #[test]
    fn clone_uniform_tile_is_cheap_and_correct() {
        let m = LayerMask::filled(two_smu(), 200);
        let coord = TileCoord {
            tile_x: 1,
            tile_y: 2,
        };
        let snap = m.clone_tile(coord);
        assert!(matches!(snap, Tile::Uniform(200)));
        assert_eq!(snap.resident_bytes(), std::mem::size_of::<Tile>());
    }

    #[test]
    fn tile_coords_overlapping_rect_returns_full_tile_set() {
        let m = LayerMask::filled(two_smu(), 0);
        // 2-SMU = 1024² → 4×4 tiles. A rect spanning (200, 200) ×
        // (400, 400) pixels touches tiles (0,0), (1,0), (0,1), (1,1).
        let coords = m.tile_coords_overlapping_rect(DirtyRect {
            x: 200,
            y: 200,
            w: 200,
            h: 200,
        });
        assert_eq!(coords.len(), 4);
        assert!(coords.contains(&TileCoord {
            tile_x: 0,
            tile_y: 0
        }));
        assert!(coords.contains(&TileCoord {
            tile_x: 1,
            tile_y: 1
        }));
    }

    #[test]
    fn restore_tile_bumps_version_for_gpu_dirty_pickup() {
        let mut m = LayerMask::filled(two_smu(), 0);
        let v0 = m.version();
        let coord = TileCoord {
            tile_x: 0,
            tile_y: 0,
        };
        m.restore_tile(coord, Tile::Uniform(128));
        assert!(
            m.version() > v0,
            "restore_tile must bump current_version so the GPU compositor re-uploads",
        );
        let dirty = m.dirty_tiles_since(v0);
        assert!(dirty.contains(&coord));
    }

    #[test]
    fn filled_layer_costs_under_one_kb_per_smu_axis() {
        // 2 SMU = 1024² → 4×4 tiles. 16 tiles × 16 bytes each + 16 ×
        // 8 bytes for the version array = 384 bytes. Well under 1 KB.
        let m = LayerMask::filled(two_smu(), 255);
        let bytes = m.resident_bytes();
        assert!(bytes < 1024, "filled layer should be < 1 KB; got {bytes}");
        assert_eq!(m.allocated_tile_count(), 0, "no pixel buffers allocated");
    }

    #[test]
    fn filled_layer_sample_returns_fill_everywhere() {
        let m = LayerMask::filled(two_smu(), 200);
        assert_eq!(m.sample(0, 0), 200);
        assert_eq!(m.sample(511, 511), 200);
        assert_eq!(m.sample(1023, 1023), 200);
        // Out-of-bounds samples return 0 (defensive — bake clips).
        assert_eq!(m.sample(1024, 0), 0);
        assert_eq!(m.sample(0, 1024), 0);
        assert_eq!(m.sample(u32::MAX, u32::MAX), 0);
    }

    #[test]
    fn write_rect_with_promotes_touched_tiles_only() {
        let mut m = LayerMask::filled(two_smu(), 0);
        // Stamp at (10, 10) of radius enough to touch only tile (0, 0).
        m.write_rect_with(10, 10, 4, 4, |_, _| 128).unwrap();
        assert_eq!(m.allocated_tile_count(), 1, "only one tile promoted");
        // Pixel inside the stamp reads back the stamped byte.
        assert_eq!(m.sample(10, 10), 128);
        assert_eq!(m.sample(13, 13), 128);
        // Pixel just outside the stamp keeps the original fill.
        assert_eq!(m.sample(9, 9), 0);
        assert_eq!(m.sample(14, 14), 0);
    }

    #[test]
    fn write_rect_with_clips_to_mask_bounds() {
        let mut m = LayerMask::filled(two_smu(), 0);
        // Stamp at (1020, 1020) of width 8 — extends 4 px past the
        // right/bottom edges. Clipped to (1020, 1020, 4, 4).
        let r = m
            .write_rect_with(1020, 1020, 8, 8, |dx, dy| 100 + (dy * 10 + dx) as u8)
            .unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (1020, 1020, 4, 4));
        // Inside the clip: pixel (1020, 1020) → f(0, 0) = 100.
        assert_eq!(m.sample(1020, 1020), 100);
        // Past the right edge: still 0 (no write).
        assert_eq!(m.sample(1023, 1020), 100 + 3);
        assert_eq!(m.sample(1023, 1023), 100 + 33);
        // Off-map sample is 0.
        assert_eq!(m.sample(1024, 1020), 0);
    }

    #[test]
    fn write_rect_with_off_map_returns_none() {
        let mut m = LayerMask::filled(two_smu(), 0);
        assert!(m.write_rect_with(2000, 2000, 4, 4, |_, _| 100).is_none());
        assert!(m.write_rect_with(0, 0, 0, 4, |_, _| 100).is_none());
        assert!(m.write_rect_with(0, 0, 4, 0, |_, _| 100).is_none());
    }

    #[test]
    fn dirty_tiles_since_returns_only_changed() {
        // Sprint 23 (H2): a freshly-filled uniform-zero mask reports
        // NO dirty tiles on cold sync — wgpu zero-initialises the
        // GPU mask array on allocation so the CPU and GPU sides
        // already match. Use `fill=255` here to keep exercising the
        // "cold-start returns every tile" half of the contract (the
        // bottom-base layer convention is `fill=255` and DOES need
        // the upload).
        let mut m = LayerMask::filled(two_smu(), 255);
        let v0 = m.version();
        let all = m.dirty_tiles_since(0);
        assert_eq!(
            all.len(),
            16,
            "non-zero-fill cold-sync uploads every tile (4×4 grid)",
        );
        // After a snapshot, no tiles are dirty until we write.
        assert!(m.dirty_tiles_since(v0).is_empty());
        // Stamp tile (0, 0).
        m.write_rect_with(10, 10, 4, 4, |_, _| 200).unwrap();
        let dirty = m.dirty_tiles_since(v0);
        assert_eq!(dirty.len(), 1);
        assert_eq!(
            dirty[0],
            TileCoord {
                tile_x: 0,
                tile_y: 0
            }
        );
        // Capture again; expect empty.
        let v1 = m.version();
        assert!(m.dirty_tiles_since(v1).is_empty());
        // Stamp tile (3, 3) — the far corner.
        m.write_rect_with(1000, 1000, 4, 4, |_, _| 200).unwrap();
        let dirty2 = m.dirty_tiles_since(v1);
        assert_eq!(dirty2.len(), 1);
        assert_eq!(
            dirty2[0],
            TileCoord {
                tile_x: 3,
                tile_y: 3
            }
        );
    }

    /// Sprint 23 (H2) — pin the post-fix contract: a freshly-filled
    /// uniform-zero mask returns ZERO dirty tiles on cold sync. The
    /// wgpu mask array zero-initialises on allocation; uploading
    /// the CPU's matching zero bytes is wasted bandwidth and the
    /// root cause of the 16-SMU PaintLayer-entry mask transfer.
    #[test]
    fn dirty_tiles_since_zero_fill_returns_empty_on_cold_sync() {
        let m = LayerMask::filled(two_smu(), 0);
        assert!(
            m.dirty_tiles_since(0).is_empty(),
            "uniform-zero mask must report ZERO dirty tiles on cold sync"
        );
        // Version is also 0 (no writes happened).
        assert_eq!(m.version(), 0);
    }

    /// Sprint 23 (H2) — pin the non-zero-fill cold-sync contract:
    /// `fill=255` (the bottom-base biome layer) reports every tile
    /// dirty on cold sync so the GPU side receives the non-zero
    /// uniform bytes.
    #[test]
    fn dirty_tiles_since_nonzero_fill_returns_all_tiles_on_cold_sync() {
        let m = LayerMask::filled(two_smu(), 255);
        assert_eq!(
            m.dirty_tiles_since(0).len(),
            16,
            "uniform-255 cold-sync must upload every tile"
        );
        assert_eq!(m.version(), 1);
    }

    #[test]
    fn dirty_tiles_since_handles_stamp_spanning_multiple_tiles() {
        let mut m = LayerMask::filled(two_smu(), 0);
        let v0 = m.version();
        // Write straddling the (0,0)/(1,0)/(0,1)/(1,1) boundary at 256,256.
        m.write_rect_with(250, 250, 12, 12, |_, _| 50).unwrap();
        let dirty = m.dirty_tiles_since(v0);
        assert_eq!(dirty.len(), 4);
        let coords: HashSet<_> = dirty.into_iter().collect();
        for (tx, ty) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert!(
                coords.contains(&TileCoord {
                    tile_x: tx,
                    tile_y: ty
                }),
                "expected dirty tile ({tx}, {ty})"
            );
        }
    }

    #[test]
    fn read_tile_uniform_path_fills_buffer_with_byte() {
        let m = LayerMask::filled(two_smu(), 77);
        let mut buf = [0u8; TILE_PIXELS];
        m.read_tile(0, 0, &mut buf);
        assert!(buf.iter().all(|&b| b == 77));
    }

    #[test]
    fn read_tile_concrete_path_returns_pixel_buffer() {
        let mut m = LayerMask::filled(two_smu(), 0);
        m.write_rect_with(10, 10, 4, 4, |dx, dy| (dy * 4 + dx) as u8)
            .unwrap();
        let mut buf = [0u8; TILE_PIXELS];
        m.read_tile(0, 0, &mut buf);
        // The 4×4 region inside tile (0, 0) starting at local (10, 10).
        for dy in 0..4 {
            for dx in 0..4 {
                let local = (10 + dy) as usize * TILE_DIM as usize + (10 + dx) as usize;
                assert_eq!(buf[local], (dy * 4 + dx) as u8);
            }
        }
        // Rest of the tile is still 0.
        assert_eq!(buf[0], 0);
        assert_eq!(buf[TILE_PIXELS - 1], 0);
    }

    #[test]
    fn tile_grid_dims_match_div_ceil() {
        let m = LayerMask::filled(MapSize::square(16), 0);
        assert_eq!(m.tile_grid_dims(), (32, 32));
        let m2 = LayerMask::filled(MapSize::square(2), 0);
        assert_eq!(m2.tile_grid_dims(), (4, 4));
    }

    // ─── Serde — round-trips ──────────────────────────────────────

    #[test]
    fn tiled_round_trip_through_toml() {
        let mut m = LayerMask::filled(two_smu(), 0);
        m.set_pixel(0, 0, 1);
        m.set_pixel(500, 500, 200);
        m.set_pixel(1023, 1023, 99);
        let s = toml::to_string(&m).unwrap();
        let m2: LayerMask = toml::from_str(&s).unwrap();
        assert_eq!(m.width, m2.width);
        assert_eq!(m.height, m2.height);
        assert_eq!(m2.sample(0, 0), 1);
        assert_eq!(m2.sample(500, 500), 200);
        assert_eq!(m2.sample(1023, 1023), 99);
        // Uniform tiles stay uniform across the round trip.
        let pre_alloc = m.allocated_tile_count();
        assert_eq!(pre_alloc, m2.allocated_tile_count());
    }

    /// Pin the Sprint-15 → Sprint-16 migration: a `.barmeproj` written
    /// by Sprint 15's `mask_bytes_b64` serializer must load cleanly
    /// AND compact the flat bytes into `Uniform` tiles.
    #[test]
    fn legacy_flat_bytes_round_trip_compresses_uniform_tiles() {
        // Synthesize a Sprint-15 wire payload manually: 1024² of 255s
        // base64-encoded as the `bytes` field. Wrap in a TOML table
        // matching `LayerMaskWire::Flat`.
        let flat = vec![255u8; 1024 * 1024];
        let b64 = BASE64.encode(&flat);
        let toml_str = format!("width = 1024\nheight = 1024\nbytes = \"{b64}\"\n");
        let m: LayerMask = toml::from_str(&toml_str).expect("legacy decode");
        assert_eq!(m.width, 1024);
        assert_eq!(m.height, 1024);
        // The fill is uniform — every tile should collapse.
        assert_eq!(
            m.allocated_tile_count(),
            0,
            "all-255 flat bytes must compact to Uniform tiles"
        );
        assert_eq!(m.sample(0, 0), 255);
        assert_eq!(m.sample(1023, 1023), 255);
    }

    /// Pin the legacy decode path's handling of mixed (non-uniform)
    /// flat bytes: at least the tiles that contain mixed content
    /// must materialise as `Pixels`.
    #[test]
    fn legacy_flat_bytes_mixed_tile_materialises_pixels() {
        let mut flat = vec![0u8; 1024 * 1024];
        // Plant a single non-zero pixel in tile (0, 0). The tile is no
        // longer uniform → it must become `Pixels`.
        flat[10 * 1024 + 10] = 99;
        let b64 = BASE64.encode(&flat);
        let toml_str = format!("width = 1024\nheight = 1024\nbytes = \"{b64}\"\n");
        let m: LayerMask = toml::from_str(&toml_str).unwrap();
        assert_eq!(m.allocated_tile_count(), 1, "only tile (0, 0) materialises");
        assert_eq!(m.sample(10, 10), 99);
        // Tile (3, 3) is still uniform 0.
        assert_eq!(m.sample(1000, 1000), 0);
    }

    #[test]
    fn legacy_short_byte_payload_pads_with_zero() {
        // Defensive: a corrupt / truncated file shouldn't crash the
        // loader. The Sprint-15 wire format had no length sentinel
        // beyond the base64 string itself.
        let flat = vec![42u8; 16]; // only 16 bytes — way short of 1024².
        let b64 = BASE64.encode(&flat);
        let toml_str = format!("width = 1024\nheight = 1024\nbytes = \"{b64}\"\n");
        let m: LayerMask = toml::from_str(&toml_str).unwrap();
        // First few bytes survive; the rest is zero.
        assert_eq!(m.sample(0, 0), 42);
        assert_eq!(m.sample(15, 0), 42);
        assert_eq!(m.sample(16, 0), 0);
        assert_eq!(m.sample(1023, 1023), 0);
    }

    /// Serialised tiled wire format must round-trip a freshly-filled
    /// mask without sandbagging — uniform tiles encode as ~5-byte
    /// strings, so the whole payload is well under 1 KB for 1024².
    #[test]
    fn tiled_uniform_only_serialisation_is_compact() {
        let m = LayerMask::filled(two_smu(), 128);
        let s = toml::to_string(&m).unwrap();
        // 16 tiles × `"u:128"` (5 bytes) + commas + quotes + array
        // syntax. Generously under 2 KB.
        assert!(
            s.len() < 2048,
            "uniform mask serialised to {} bytes",
            s.len()
        );
    }

    // ─── Helpers — flood_fill ─────────────────────────────────────

    #[test]
    fn flood_fill_within_threshold_paints_connected_region() {
        let mut m = LayerMask::filled(two_smu(), 100);
        // Plant a small disjoint patch at (500, 500) with value 200.
        for y in 498..=502 {
            for x in 498..=502 {
                m.set_pixel(x, y, 200);
            }
        }
        // Flood from inside the patch with threshold 5 → fills the
        // 200-region only.
        let bbox = flood_fill(&mut m, 500, 500, 0, 5).expect("filled some pixels");
        // The 5×5 patch becomes 0; surrounding 100s untouched.
        for y in 498..=502 {
            for x in 498..=502 {
                assert_eq!(m.sample(x, y), 0, "patch pixel ({x}, {y}) not flooded");
            }
        }
        assert_eq!(m.sample(497, 500), 100);
        assert_eq!(m.sample(503, 500), 100);
        // Bounding box covers just the patch.
        assert_eq!(bbox.x, 498);
        assert_eq!(bbox.y, 498);
        assert_eq!(bbox.w, 5);
        assert_eq!(bbox.h, 5);
    }

    #[test]
    fn flood_fill_off_map_returns_none() {
        let mut m = LayerMask::filled(two_smu(), 0);
        assert!(flood_fill(&mut m, 2000, 2000, 255, 5).is_none());
    }

    #[test]
    fn flood_fill_target_equal_to_seed_still_visits_region() {
        // Flooding a uniform mask with target = current does nothing
        // observable in the bytes BUT does bump the version (writes
        // happen even though the value is unchanged). That's fine —
        // the GPU upload will re-upload the same bytes, which is
        // wasted bandwidth but not a correctness bug. Pin the
        // observable behaviour.
        let mut m = LayerMask::filled(two_smu(), 100);
        let bbox = flood_fill(&mut m, 500, 500, 100, 5);
        assert!(bbox.is_some(), "uniform flood still reports a bbox");
    }
}
