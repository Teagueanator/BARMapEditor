//! Splat distribution + brush trait (D3 / Sprint 8).
//!
//! Models the RGBA "splat distribution texture" sampled by the SMF
//! fragment shader at `uv ∈ [0,1]^2` over the whole map (see
//! `RecoilEngine/cont/base/springcontent/shaders/GLSL/SMFFragProg.glsl:177`
//! and `docs/research/source-audit-2026-05-18/FINDINGS.md` §7.3). The
//! pixel dimension is fixed at [`SPLAT_DIM`]² regardless of map size:
//! the engine is resolution-flexible at this texture (`titanduel`
//! ships 1024², `comet` ships 2048×1024 at ~half-axis-resolution vs
//! its SMT diffuse) so the editor picks one number and sticks with
//! it. 1024² × 4 = 4 MB resident, a comfortable middle vs the
//! heightmap. Larger map → coarser per-pixel coverage; the
//! visible-tile size is driven by `splats.texScales` in the emitter
//! (D6 / Sprint 12), not by this resolution.
//!
//! The data model mirrors [`crate::brushes`] (ADR-018):
//! - [`SplatDistribution`] holds the RGBA8 pixel array.
//! - [`SplatStamp`] is one stamp in a brush stroke (world-space).
//! - [`SplatBrush`] is the object-safe `Send + Sync + 'static` plugin
//!   surface; [`SplatBrushRegistry`] is the registry.
//! - Three starter brushes — [`PaintChannel`], [`Erase`], [`Smooth`].
//!
//! Brushes follow the dirty-rect upload pattern from ADR-018: compute
//! the affected pixel bbox, walk only those pixels, return the bbox
//! so the GPU upload (D4 / Sprint 9) only re-uploads the changed
//! rect.
//!
//! Channel ordering ([`SplatChannel`]) is `R, G, B, A` to match the
//! inspector's row order (D5 / Sprint 9). Brush ids (`paint`, `erase`,
//! `smooth`) match the BRUSH-section chip ids the inspector dispatches
//! on at stamp time.
//!
//! TODO(splat-undo): the distribution is 4 MB at 1024² — too large
//! to snapshot per stroke against the existing 100 MB undo cap
//! (would evict ~25 heightmap strokes per splat stroke). Defer to a
//! follow-up that adapts A1's bitset copy-on-first-write pattern.
//! Until then, splat edits are not undoable. See
//! `devlog/stage-1-mvp/phase-3-plan.md` D3 "No undo integration".

use crate::MapSize;
use crate::brushes::{DirtyRect, smoothstep};
use serde::{Deserialize, Serialize};

/// Fixed pixel side of the splat distribution texture. See module
/// docs for the rationale.
pub const SPLAT_DIM: u32 = 1024;

/// Persisted per-channel splat config (D5 / Sprint 9). Maps directly
/// to the `mapinfo.splats` block and the `mapinfo.resources.
/// splatDetailNormalTex[]` subtable form (D6 emission, Sprint 12).
///
/// `channels[i] = Some(slot_id)` binds slot `slot_id` (an index into
/// the `tools/textures/<NN-slot>/` registry — D1 / ADR-027) to the
/// corresponding RGBA channel of the splat distribution:
/// - channels[0] → R
/// - channels[1] → G
/// - channels[2] → B
/// - channels[3] → A
///
/// `None` = the channel is unbound. The GPU shader gates per-layer
/// sampling on an active-slot mask derived from these `Option`s
/// (D4 / ADR-036).
///
/// `tex_scales` / `tex_mults` mirror `mapinfo.splats.texScales` /
/// `texMults` exactly — float4, defaults `0.02` / `1.0` per FINDINGS
/// §1.6.
///
/// `diffuse_in_alpha` mirrors `mapinfo.resources.
/// splatDetailNormalDiffuseAlpha`. ADR-034 is the open ADR for the
/// high-pass diffuse-offset workflow; baseline is `false` per
/// ADR-025.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplatConfig {
    /// Per-channel slot bindings. Serialized as a 4-element array of
    /// signed ints (`-1` = unbound, `0..=255` = slot id) because TOML
    /// arrays have no null variant. The Rust API stays `Option<u8>`
    /// for type safety; the wire conversion lives in [`channels_wire`].
    #[serde(with = "channels_wire")]
    pub channels: [Option<u8>; 4],
    pub tex_scales: [f32; 4],
    pub tex_mults: [f32; 4],
    #[serde(default)]
    pub diffuse_in_alpha: bool,
}

/// Serde shim for `[Option<u8>; 4]` ↔ `[i16; 4]`. TOML arrays cannot
/// hold a null sentinel, so we encode `None` as `-1`. `u8::MAX` would
/// be ambiguous with a real slot id once the registry exceeds 256
/// entries (unlikely but the math is cheap).
mod channels_wire {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[Option<u8>; 4], s: S) -> Result<S::Ok, S::Error> {
        let wire: [i16; 4] = [
            v[0].map(|x| x as i16).unwrap_or(-1),
            v[1].map(|x| x as i16).unwrap_or(-1),
            v[2].map(|x| x as i16).unwrap_or(-1),
            v[3].map(|x| x as i16).unwrap_or(-1),
        ];
        wire.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[Option<u8>; 4], D::Error> {
        let wire: [i16; 4] = <[i16; 4]>::deserialize(d)?;
        let mut out = [None; 4];
        for (o, w) in out.iter_mut().zip(wire.iter()) {
            *o = if *w < 0 { None } else { Some(*w as u8) };
        }
        Ok(out)
    }
}

impl Default for SplatConfig {
    fn default() -> Self {
        // Defaults match `splats.texScales = vec4(0.02)` and
        // `splats.texMults = vec4(1.0)` from MapInfo.cpp::ReadSplats
        // (FINDINGS §1.6). All channels unbound — fresh projects
        // render the fallback gradient until the user paints.
        Self {
            channels: [None; 4],
            tex_scales: [0.02; 4],
            tex_mults: [1.0; 4],
            diffuse_in_alpha: false,
        }
    }
}

impl SplatConfig {
    /// Bit `i` set iff `channels[i].is_some()`. The D4 shader gates
    /// per-layer sampling on this mask.
    pub fn active_mask(&self) -> u32 {
        let mut m = 0u32;
        for (i, c) in self.channels.iter().enumerate() {
            if c.is_some() {
                m |= 1 << i;
            }
        }
        m
    }
}

/// RGBA splat distribution. Channel weights — not transparency —
/// drive which DNTS slot lights each fragment. The engine multiplies
/// the per-channel sample by `splats.texMults[i]` then saturates the
/// total to `≤ 1.0`; the editor's normalisation rule (per
/// [`PaintChannel`]) keeps `R + G + B + A ≤ 255` so the live preview
/// matches the engine's downstream behaviour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplatDistribution {
    pub width: u32,
    pub height: u32,
    /// Map size in SMU. Drives world→pixel conversion in brushes:
    /// `pixel_x = world_x * width / elmo_extent_x`.
    pub map_size: MapSize,
    pub rgba: Vec<[u8; 4]>,
}

/// One of the four splat distribution channels. Order matches the
/// inspector's TEXTURE LAYERS row order (D5 / Sprint 9) so the App's
/// `splat_brush_state.active_channel` indexes into this enum directly.
///
/// Serializes as the bare channel letter ("R", "G", "B", "A") for
/// readable round-trips through TOML (D8 / Sprint 15 — used by
/// [`crate::layers::TextureLayer::dnts_channel`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SplatChannel {
    R,
    G,
    B,
    A,
}

impl SplatChannel {
    /// Byte index of this channel within an `[u8; 4]` RGBA pixel.
    pub const fn index(self) -> usize {
        match self {
            Self::R => 0,
            Self::G => 1,
            Self::B => 2,
            Self::A => 3,
        }
    }
}

/// One stamp in a brush stroke. World coordinates are in elmos
/// (matches [`crate::brushes::BrushStamp`]).
#[derive(Debug, Clone, Copy)]
pub struct SplatStamp {
    /// Stamp center, world X (elmos). 0 = west edge of the map.
    pub world_x: f32,
    /// Stamp center, world Z (elmos). 0 = north edge of the map.
    pub world_z: f32,
    /// Brush radius (elmos).
    pub radius: f32,
    /// Strength 0..=1. Interpretation is brush-specific.
    pub strength: f32,
    /// Target channel. Used by [`PaintChannel`] and [`Erase`]; the
    /// smoothing brush mixes all four channels and ignores this.
    pub channel: SplatChannel,
}

/// Plugin surface for splat brushes. Mirrors [`crate::brushes::Brush`]
/// — object-safe, `Send + Sync + 'static` so a future wasm-plugin
/// runtime could hand back `Box<dyn SplatBrush>` from outside this
/// crate.
pub trait SplatBrush: Send + Sync + 'static {
    /// Stable id used as the serialization key + UI lookup. Lowercase
    /// kebab ascii. The inspector's brush-mode buttons match on this.
    fn id(&self) -> &'static str;

    /// Display label for UI dropdowns.
    fn label(&self) -> &'static str;

    /// Apply one stamp. Returns the pixel bounding box that changed,
    /// or `None` if the stamp was wholly outside the texture /
    /// zero-radius / zero-strength.
    fn apply(&self, dist: &mut SplatDistribution, stamp: SplatStamp) -> Option<DirtyRect>;
}

/// Vector of `Box<dyn SplatBrush>` — same shape as
/// [`crate::brushes::BrushRegistry`]. Built once at app start; the
/// splat-tool UI (D5 / Sprint 9) iterates to populate brush-mode
/// chips and looks up by id at stamp time.
pub struct SplatBrushRegistry {
    brushes: Vec<Box<dyn SplatBrush>>,
}

impl SplatBrushRegistry {
    /// Ships with `paint` / `erase` / `smooth`. New splat brushes
    /// drop in here as `impl SplatBrush` + one line.
    pub fn default_set() -> Self {
        Self {
            brushes: vec![Box::new(PaintChannel), Box::new(Erase), Box::new(Smooth)],
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn SplatBrush> {
        self.brushes.iter().map(|b| b.as_ref())
    }

    pub fn get(&self, id: &str) -> Option<&dyn SplatBrush> {
        self.brushes
            .iter()
            .find(|b| b.id() == id)
            .map(|b| b.as_ref())
    }

    pub fn is_empty(&self) -> bool {
        self.brushes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.brushes.len()
    }
}

impl Default for SplatBrushRegistry {
    fn default() -> Self {
        Self::default_set()
    }
}

impl SplatDistribution {
    /// Allocate a fresh distribution at [`SPLAT_DIM`]² for the given
    /// map size. All channels initialised to 0. D6 (Sprint 12) will
    /// emit a "all-R=255" texture at build time when the distribution
    /// is empty, so unpainted maps render the slot-0 base rather
    /// than going black.
    pub fn new(map_size: MapSize) -> Self {
        let len = (SPLAT_DIM * SPLAT_DIM) as usize;
        Self {
            width: SPLAT_DIM,
            height: SPLAT_DIM,
            map_size,
            rgba: vec![[0; 4]; len],
        }
    }

    /// Read pixel at `(x, y)` in pixel coordinates. Returns `None`
    /// if out of bounds.
    pub fn get(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.rgba[(y * self.width + x) as usize])
    }

    /// Elmos per splat pixel along X / Z. Anisotropic when the map
    /// is non-square (currently `MapSize` is always square but the
    /// math is general).
    fn elmos_per_pixel(&self) -> (f32, f32) {
        let (ex, ez) = self.map_size.elmo_extents();
        (
            ex as f32 / self.width as f32,
            ez as f32 / self.height as f32,
        )
    }
}

/// Pixel bounding box of a stamp clipped to the texture. Same shape
/// as [`crate::brushes::pixel_bbox`] but in splat-texel space.
fn splat_pixel_bbox(dist: &SplatDistribution, stamp: SplatStamp) -> Option<DirtyRect> {
    if dist.width == 0 || dist.height == 0 {
        return None;
    }
    let (epx, epz) = dist.elmos_per_pixel();
    let r_px_x = (stamp.radius / epx).max(0.0);
    let r_px_z = (stamp.radius / epz).max(0.0);
    if r_px_x <= 0.0 || r_px_z <= 0.0 {
        return None;
    }
    let cx = stamp.world_x / epx;
    let cz = stamp.world_z / epz;
    let min_x = (cx - r_px_x).floor().max(0.0) as i64;
    let max_x = (cx + r_px_x).ceil().min((dist.width - 1) as f32) as i64;
    let min_y = (cz - r_px_z).floor().max(0.0) as i64;
    let max_y = (cz + r_px_z).ceil().min((dist.height - 1) as f32) as i64;
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

/// Per-pixel `weight = strength × smoothstep(1 - d/r)` where `d` is
/// the elmo distance from the stamp centre, normalised by `r` so the
/// kernel is circular even when X and Z resolve to different
/// elmos-per-pixel ratios.
struct Falloff {
    cx_px: f32,
    cz_px: f32,
    r_px: f32,
    /// Per-axis scale to normalise pixel-space `dz` against `dx` so
    /// the distance is isotropic in elmo space.
    z_scale: f32,
    strength: f32,
}

impl Falloff {
    fn from(dist: &SplatDistribution, stamp: SplatStamp) -> Self {
        let (epx, epz) = dist.elmos_per_pixel();
        let r_px = stamp.radius / epx;
        let z_scale = epz / epx; // multiply pixel-space dz to convert into "x-equivalent" units
        Self {
            cx_px: stamp.world_x / epx,
            cz_px: stamp.world_z / epz,
            r_px: r_px.max(f32::EPSILON),
            z_scale,
            strength: stamp.strength.clamp(0.0, 1.0),
        }
    }

    /// Returns the falloff-weighted strength at pixel `(ix, iz)`, or
    /// `None` if outside the circular kernel.
    fn weight_at(&self, ix: u32, iz: u32) -> Option<f32> {
        let dx = ix as f32 - self.cx_px;
        let dz = (iz as f32 - self.cz_px) * self.z_scale;
        let d = (dx * dx + dz * dz).sqrt();
        if d > self.r_px {
            return None;
        }
        let w = self.strength * smoothstep(1.0 - d / self.r_px);
        (w > 0.0).then_some(w)
    }
}

// ----- brushes --------------------------------------------------------------

/// Paint towards 255 on the stamp's channel with smoothstep falloff,
/// clamping the other three channels down so `R + G + B + A ≤ 255`
/// stays satisfied (BAR's normalisation rule).
pub struct PaintChannel;

impl SplatBrush for PaintChannel {
    fn id(&self) -> &'static str {
        "paint"
    }
    fn label(&self) -> &'static str {
        "Paint"
    }

    fn apply(&self, dist: &mut SplatDistribution, stamp: SplatStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 || stamp.radius <= 0.0 {
            return None;
        }
        let bbox = splat_pixel_bbox(dist, stamp)?;
        let falloff = Falloff::from(dist, stamp);
        let ch = stamp.channel.index();
        for iz in bbox.y..bbox.y + bbox.h {
            for ix in bbox.x..bbox.x + bbox.w {
                let Some(weight) = falloff.weight_at(ix, iz) else {
                    continue;
                };
                let idx = (iz * dist.width + ix) as usize;
                paint_channel_pixel(&mut dist.rgba[idx], ch, weight);
            }
        }
        Some(bbox)
    }
}

/// In-place paint of one pixel. Move the target channel toward 255
/// proportionally to `weight`, then scale the other three channels
/// down (floor) to preserve `R + G + B + A ≤ 255`.
fn paint_channel_pixel(px: &mut [u8; 4], ch: usize, weight: f32) {
    let cur = f32::from(px[ch]);
    let new_ch_f = cur + (255.0 - cur) * weight;
    let new_ch = new_ch_f.round().clamp(0.0, 255.0) as u8;
    if new_ch == px[ch] {
        return;
    }
    let budget = 255u32.saturating_sub(u32::from(new_ch));
    let others_sum: u32 = (0..4).filter(|&i| i != ch).map(|i| u32::from(px[i])).sum();
    if others_sum > budget && others_sum > 0 {
        let scale = budget as f32 / others_sum as f32;
        for (i, p) in px.iter_mut().enumerate() {
            if i == ch {
                continue;
            }
            // floor() guarantees we stay within budget after rounding.
            *p = (f32::from(*p) * scale).floor() as u8;
        }
    }
    px[ch] = new_ch;
    // Defensive double-check — should always hold given the floor()
    // scaling above. debug_assert so release builds skip it.
    debug_assert!(
        u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]) + u32::from(px[3]) <= 255,
        "channel sum invariant"
    );
}

/// Move the stamp's channel toward 0 with smoothstep falloff. Other
/// channels untouched.
pub struct Erase;

impl SplatBrush for Erase {
    fn id(&self) -> &'static str {
        "erase"
    }
    fn label(&self) -> &'static str {
        "Erase"
    }

    fn apply(&self, dist: &mut SplatDistribution, stamp: SplatStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 || stamp.radius <= 0.0 {
            return None;
        }
        let bbox = splat_pixel_bbox(dist, stamp)?;
        let falloff = Falloff::from(dist, stamp);
        let ch = stamp.channel.index();
        for iz in bbox.y..bbox.y + bbox.h {
            for ix in bbox.x..bbox.x + bbox.w {
                let Some(weight) = falloff.weight_at(ix, iz) else {
                    continue;
                };
                let idx = (iz * dist.width + ix) as usize;
                let cur = f32::from(dist.rgba[idx][ch]);
                let new = (cur * (1.0 - weight)).round().clamp(0.0, 255.0) as u8;
                dist.rgba[idx][ch] = new;
            }
        }
        Some(bbox)
    }
}

/// 3×3 mean blend per pixel across all four channels, lerped toward
/// the average by `strength × falloff`. Reads from a snapshot of the
/// bounding rect (+1-pixel margin) so propagation doesn't bias the
/// pass. Mirrors [`crate::brushes::Smooth`] on the heightmap.
pub struct Smooth;

impl SplatBrush for Smooth {
    fn id(&self) -> &'static str {
        "smooth"
    }
    fn label(&self) -> &'static str {
        "Smooth"
    }

    fn apply(&self, dist: &mut SplatDistribution, stamp: SplatStamp) -> Option<DirtyRect> {
        if stamp.strength <= 0.0 || stamp.radius <= 0.0 {
            return None;
        }
        let bbox = splat_pixel_bbox(dist, stamp)?;
        let falloff = Falloff::from(dist, stamp);

        let snap_x = bbox.x.saturating_sub(1);
        let snap_y = bbox.y.saturating_sub(1);
        let snap_r = (bbox.x + bbox.w + 1).min(dist.width);
        let snap_b = (bbox.y + bbox.h + 1).min(dist.height);
        let snap_w = snap_r - snap_x;
        let snap_h = snap_b - snap_y;
        let mut snap: Vec<[u8; 4]> = Vec::with_capacity((snap_w * snap_h) as usize);
        for iz in snap_y..snap_b {
            let row_start = (iz * dist.width + snap_x) as usize;
            snap.extend_from_slice(&dist.rgba[row_start..row_start + snap_w as usize]);
        }

        for iz in bbox.y..bbox.y + bbox.h {
            for ix in bbox.x..bbox.x + bbox.w {
                let Some(weight) = falloff.weight_at(ix, iz) else {
                    continue;
                };
                let xlo = ix.saturating_sub(1).max(snap_x);
                let xhi = (ix + 1).min(snap_r - 1);
                let zlo = iz.saturating_sub(1).max(snap_y);
                let zhi = (iz + 1).min(snap_b - 1);
                let mut sum = [0u32; 4];
                let mut count = 0u32;
                for nz in zlo..=zhi {
                    for nx in xlo..=xhi {
                        let lx = (nx - snap_x) as usize;
                        let lz = (nz - snap_y) as usize;
                        let n = snap[lz * snap_w as usize + lx];
                        for c in 0..4 {
                            sum[c] += u32::from(n[c]);
                        }
                        count += 1;
                    }
                }
                if count == 0 {
                    continue;
                }
                let idx = (iz * dist.width + ix) as usize;
                let cur = dist.rgba[idx];
                let mut new_px = [0u8; 4];
                for c in 0..4 {
                    let avg = sum[c] as f32 / count as f32;
                    let mix = f32::from(cur[c]) * (1.0 - weight) + avg * weight;
                    new_px[c] = mix.round().clamp(0.0, 255.0) as u8;
                }
                dist.rgba[idx] = new_px;
            }
        }
        Some(bbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist_16smu() -> SplatDistribution {
        SplatDistribution::new(MapSize::square(16))
    }

    fn pixel_sum(px: [u8; 4]) -> u32 {
        u32::from(px[0]) + u32::from(px[1]) + u32::from(px[2]) + u32::from(px[3])
    }

    #[test]
    fn distribution_new_allocates_one_megapixel() {
        let d = dist_16smu();
        assert_eq!(d.width, 1024);
        assert_eq!(d.height, 1024);
        assert_eq!(d.rgba.len(), 1024 * 1024);
        // Backing buffer is exactly 4 MB.
        assert_eq!(
            d.rgba.len() * std::mem::size_of::<[u8; 4]>(),
            4 * 1024 * 1024
        );
        for px in &d.rgba {
            assert_eq!(*px, [0, 0, 0, 0]);
        }
    }

    #[test]
    fn registry_ships_three_brushes_with_kebab_ids() {
        let r = SplatBrushRegistry::default_set();
        assert_eq!(r.len(), 3);
        for id in ["paint", "erase", "smooth"] {
            assert!(r.get(id).is_some(), "missing {id}");
        }
        assert!(r.get("nonexistent").is_none());
    }

    #[test]
    fn splat_channel_indices_match_inspector_row_order() {
        // The inspector lists rows R, G, B, A (top→bottom). Indices
        // must match so D5 can read `splat_state.layers[c.index()]`.
        assert_eq!(SplatChannel::R.index(), 0);
        assert_eq!(SplatChannel::G.index(), 1);
        assert_eq!(SplatChannel::B.index(), 2);
        assert_eq!(SplatChannel::A.index(), 3);
    }

    #[test]
    fn paint_g_center_writes_255_on_g_only() {
        let mut d = dist_16smu();
        // 16-SMU map covers 0..8192 elmos. Centre = (4096, 4096).
        let stamp = SplatStamp {
            world_x: 4096.0,
            world_z: 4096.0,
            radius: 100.0,
            strength: 1.0,
            channel: SplatChannel::G,
        };
        let bbox = PaintChannel
            .apply(&mut d, stamp)
            .expect("must touch pixels");
        // Centre pixel: world (4096, 4096) → (4096/8 = 512, 512).
        let center = d.get(512, 512).unwrap();
        assert_eq!(center[1], 255, "G saturated");
        assert_eq!(center[0], 0, "R untouched (was 0)");
        assert_eq!(center[2], 0, "B untouched");
        assert_eq!(center[3], 0, "A untouched");
        assert!(bbox.w > 0 && bbox.h > 0);
        // bbox must contain pixel (512, 512).
        assert!(bbox.x <= 512 && 512 < bbox.x + bbox.w);
        assert!(bbox.y <= 512 && 512 < bbox.y + bbox.h);
    }

    #[test]
    fn paint_then_erase_returns_channel_to_zero_within_tolerance() {
        let mut d = dist_16smu();
        let world = (4096.0, 4096.0);
        let paint = SplatStamp {
            world_x: world.0,
            world_z: world.1,
            radius: 80.0,
            strength: 1.0,
            channel: SplatChannel::R,
        };
        PaintChannel.apply(&mut d, paint).unwrap();
        let after_paint = d.get(512, 512).unwrap();
        assert!(after_paint[0] >= 250, "centre R painted");

        // Multiple erase stamps to drive R back near 0 (smoothstep
        // means a single stamp leaves a residual at the centre too).
        let erase = SplatStamp {
            world_x: world.0,
            world_z: world.1,
            radius: 80.0,
            strength: 1.0,
            channel: SplatChannel::R,
        };
        for _ in 0..8 {
            Erase.apply(&mut d, erase).unwrap();
        }
        let after_erase = d.get(512, 512).unwrap();
        assert!(
            after_erase[0] <= 2,
            "centre R returns to near zero after multiple erases: got {}",
            after_erase[0]
        );
    }

    #[test]
    fn smooth_reduces_local_variance_around_a_spike() {
        let mut d = dist_16smu();
        // Spike at (512, 512): G = 255, surrounded by 0s.
        d.rgba[512 * 1024 + 512] = [0, 255, 0, 0];
        let before = local_variance_g(&d, 512, 512, 4);
        let stamp = SplatStamp {
            world_x: 4096.0,
            world_z: 4096.0,
            radius: 80.0,
            strength: 1.0,
            channel: SplatChannel::G, // smooth ignores this; keep valid
        };
        for _ in 0..6 {
            Smooth.apply(&mut d, stamp).unwrap();
        }
        let after = local_variance_g(&d, 512, 512, 4);
        assert!(
            after < before,
            "smoothing reduces variance: before={before:.0}, after={after:.0}"
        );
    }

    fn local_variance_g(d: &SplatDistribution, cx: u32, cz: u32, r: u32) -> f64 {
        let mut sum = 0.0f64;
        let mut sq = 0.0f64;
        let mut n = 0.0f64;
        for iz in cz.saturating_sub(r)..=(cz + r).min(d.height - 1) {
            for ix in cx.saturating_sub(r)..=(cx + r).min(d.width - 1) {
                let v = f64::from(d.get(ix, iz).unwrap()[1]);
                sum += v;
                sq += v * v;
                n += 1.0;
            }
        }
        let mean = sum / n;
        sq / n - mean * mean
    }

    #[test]
    fn channel_sum_invariant_holds_after_paint_against_loaded_pixel() {
        let mut d = dist_16smu();
        // Pre-load the centre pixel with 100/100/55/0 (sum = 255).
        let cx = 512;
        let cz = 512;
        d.rgba[(cz * 1024 + cx) as usize] = [100, 100, 55, 0];
        let stamp = SplatStamp {
            world_x: 4096.0,
            world_z: 4096.0,
            radius: 80.0,
            strength: 0.5,
            channel: SplatChannel::B,
        };
        PaintChannel.apply(&mut d, stamp).unwrap();
        // Centre + a sample inside the brush must still satisfy R+G+B+A ≤ 255.
        for (ix, iz) in [(512u32, 512u32), (508, 512), (515, 515), (510, 514)] {
            let px = d.get(ix, iz).unwrap();
            assert!(
                pixel_sum(px) <= 255,
                "channel sum invariant violated at ({ix},{iz}): {px:?} sums to {}",
                pixel_sum(px)
            );
        }
    }

    #[test]
    fn channel_sum_invariant_holds_against_loaded_neighbours() {
        // Stress: 100 random-ish prepopulated pixels under a wide
        // brush, then PaintChannel a different channel. None of the
        // touched pixels may exceed 255 after.
        let mut d = dist_16smu();
        let preset = [
            [200u8, 30, 25, 0],
            [50, 50, 50, 50],
            [255, 0, 0, 0],
            [128, 64, 32, 31],
            [0, 0, 0, 255],
        ];
        for i in 0..100 {
            let ix = 500 + (i % 10);
            let iz = 500 + (i / 10);
            d.rgba[(iz * 1024 + ix) as usize] = preset[i as usize % preset.len()];
        }
        let stamp = SplatStamp {
            world_x: 4096.0,
            world_z: 4096.0,
            radius: 200.0,
            strength: 0.4,
            channel: SplatChannel::A,
        };
        PaintChannel.apply(&mut d, stamp).unwrap();
        for iz in 495..520 {
            for ix in 495..520 {
                let px = d.get(ix, iz).unwrap();
                assert!(
                    pixel_sum(px) <= 255,
                    "invariant: ({ix},{iz}) {px:?} sums to {}",
                    pixel_sum(px)
                );
            }
        }
    }

    #[test]
    fn paint_off_map_returns_none() {
        let mut d = dist_16smu();
        let stamp = SplatStamp {
            world_x: -1000.0,
            world_z: -1000.0,
            radius: 50.0,
            strength: 1.0,
            channel: SplatChannel::R,
        };
        assert!(PaintChannel.apply(&mut d, stamp).is_none());
    }

    #[test]
    fn zero_strength_returns_none() {
        let mut d = dist_16smu();
        for brush in SplatBrushRegistry::default_set().iter() {
            let stamp = SplatStamp {
                world_x: 4096.0,
                world_z: 4096.0,
                radius: 80.0,
                strength: 0.0,
                channel: SplatChannel::R,
            };
            assert!(brush.apply(&mut d, stamp).is_none(), "{}", brush.id());
        }
    }

    #[test]
    fn zero_radius_returns_none() {
        let mut d = dist_16smu();
        for brush in SplatBrushRegistry::default_set().iter() {
            let stamp = SplatStamp {
                world_x: 4096.0,
                world_z: 4096.0,
                radius: 0.0,
                strength: 1.0,
                channel: SplatChannel::R,
            };
            assert!(brush.apply(&mut d, stamp).is_none(), "{}", brush.id());
        }
    }

    #[test]
    fn get_returns_none_out_of_bounds() {
        let d = dist_16smu();
        assert!(d.get(0, 0).is_some());
        assert!(d.get(1023, 1023).is_some());
        assert!(d.get(1024, 0).is_none());
        assert!(d.get(0, 1024).is_none());
        assert!(d.get(u32::MAX, 0).is_none());
    }

    #[test]
    fn registry_iter_ships_brushes_in_declared_order() {
        let r = SplatBrushRegistry::default_set();
        let ids: Vec<_> = r.iter().map(|b| b.id()).collect();
        assert_eq!(ids, vec!["paint", "erase", "smooth"]);
        let labels: Vec<_> = r.iter().map(|b| b.label()).collect();
        assert_eq!(labels, vec!["Paint", "Erase", "Smooth"]);
        assert!(!r.is_empty());
    }

    #[test]
    fn paint_dirty_rect_contains_center_and_clips_to_texture() {
        let mut d = dist_16smu();
        let stamp = SplatStamp {
            world_x: 0.0, // corner — should clip rect to (0..) only
            world_z: 0.0,
            radius: 100.0,
            strength: 1.0,
            channel: SplatChannel::R,
        };
        let bbox = PaintChannel.apply(&mut d, stamp).expect("touches pixels");
        assert!(bbox.x + bbox.w <= d.width, "rect clipped to texture");
        assert!(bbox.y + bbox.h <= d.height);
        // Corner pixel (0,0) should be in the rect.
        assert!(bbox.x == 0 && bbox.y == 0);
        // Stamp at (0,0) world → centre pixel (0,0); paint must touch it.
        let corner = d.get(0, 0).unwrap();
        assert!(corner[0] > 0, "corner R painted: {corner:?}");
    }

    #[test]
    fn erase_does_not_touch_other_channels() {
        let mut d = dist_16smu();
        // Pre-load a pixel with non-zero everywhere.
        let cx = 512u32;
        let cz = 512u32;
        d.rgba[(cz * 1024 + cx) as usize] = [80, 80, 80, 15];
        let stamp = SplatStamp {
            world_x: 4096.0,
            world_z: 4096.0,
            radius: 80.0,
            strength: 1.0,
            channel: SplatChannel::R,
        };
        Erase.apply(&mut d, stamp).unwrap();
        let px = d.get(cx, cz).unwrap();
        // Only R should have moved toward 0; others unchanged.
        assert!(px[0] < 80, "R erased: {px:?}");
        assert_eq!(px[1], 80, "G untouched by erase(R)");
        assert_eq!(px[2], 80, "B untouched by erase(R)");
        assert_eq!(px[3], 15, "A untouched by erase(R)");
    }

    #[test]
    fn smooth_preserves_channel_sum_invariant() {
        // Smooth's output is a linear combination of inputs; if every
        // input pixel obeys R+G+B+A ≤ 255 then so does the output.
        // Stress: paint a region (PaintChannel maintains the
        // invariant), then smooth over it, then confirm.
        let mut d = dist_16smu();
        for (i, ch) in [SplatChannel::R, SplatChannel::G, SplatChannel::B]
            .iter()
            .enumerate()
        {
            let stamp = SplatStamp {
                world_x: 4096.0 + i as f32 * 50.0,
                world_z: 4096.0,
                radius: 80.0,
                strength: 0.5,
                channel: *ch,
            };
            PaintChannel.apply(&mut d, stamp).unwrap();
        }
        // Smooth pass over the painted area.
        let smooth_stamp = SplatStamp {
            world_x: 4096.0,
            world_z: 4096.0,
            radius: 200.0,
            strength: 1.0,
            channel: SplatChannel::R, // ignored by smooth
        };
        let bbox = Smooth.apply(&mut d, smooth_stamp).unwrap();
        for iz in bbox.y..bbox.y + bbox.h {
            for ix in bbox.x..bbox.x + bbox.w {
                let px = d.get(ix, iz).unwrap();
                let s = pixel_sum(px);
                assert!(
                    s <= 255,
                    "smooth violated invariant at ({ix},{iz}): {px:?} sums to {s}"
                );
            }
        }
    }

    #[test]
    fn splat_dim_constant_is_one_thousand_twenty_four() {
        // The fixed 1024² dim is a public contract (D4 will allocate
        // the GPU texture at this size); pin it as a test rather than
        // a doc-only assertion so a typo can't silently break things.
        assert_eq!(SPLAT_DIM, 1024);
    }

    #[test]
    fn splat_config_default_matches_engine_defaults() {
        // FINDINGS §1.6 — splats default texScales=0.02, texMults=1.0.
        let c = SplatConfig::default();
        assert_eq!(c.channels, [None; 4]);
        assert_eq!(c.tex_scales, [0.02; 4]);
        assert_eq!(c.tex_mults, [1.0; 4]);
        // ADR-025 baseline — diffuse_in_alpha workflow stays off
        // until ADR-034 lands.
        assert!(!c.diffuse_in_alpha);
    }

    #[test]
    fn splat_config_active_mask_reflects_bound_channels() {
        let mut c = SplatConfig::default();
        assert_eq!(c.active_mask(), 0);
        c.channels[0] = Some(5);
        c.channels[2] = Some(3);
        // R + B bound → bits 0 and 2 = 0b101 = 5.
        assert_eq!(c.active_mask(), 0b101);
        c.channels[3] = Some(9);
        assert_eq!(c.active_mask(), 0b1101);
    }

    #[test]
    fn splat_config_round_trips_through_toml() {
        let c = SplatConfig {
            channels: [Some(0), Some(2), None, Some(8)],
            tex_scales: [0.02, 0.004, 0.02, 0.0015],
            tex_mults: [1.0, 1.5, 1.0, 0.8],
            diffuse_in_alpha: true,
        };
        let s = toml::to_string(&c).unwrap();
        let c2: SplatConfig = toml::from_str(&s).unwrap();
        assert_eq!(c, c2);
    }
}
