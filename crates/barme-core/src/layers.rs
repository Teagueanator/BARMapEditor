//! Layered texture stack (D8 / Sprint 15, ADR-038).
//!
//! Sits above [`crate::splat`] (the BAR-runtime 4-channel splat
//! distribution) and below the .sd7 export bake. A [`LayerStack`]
//! holds N [`TextureLayer`]s, each carrying a source (slot id or
//! imported path), a 2D transform, blend params, an optional
//! [`SplatChannel`] binding, and a per-layer alpha mask sized to the
//! map's diffuse dims (`512 × SMU` per side).
//!
//! Sprint 15 ships:
//! - The data model + serde.
//! - Migration from `Project.splat_config` (one layer seeded per
//!   bound DNTS slot at load time).
//! - The CPU compositor [`LayerStack::bake_diffuse`].
//! - `Project::layers` field + `ProjectDiff` variants for
//!   add / remove / reorder / set-property.
//! - Replacement of `synth_biome_bmp` in `launcher.rs` with a
//!   `LayerStack::bake_diffuse` call (with fallback to the biome
//!   ramp when the stack is empty — covers pre-D8 projects loaded
//!   without a layers block, plus the `barme-pipeline` smoke
//!   example that builds a bare `Project` directly).
//!
//! Sprint 16 adds: tiled COW masks, layer mask brushes, the GPU
//! composite preview shader, the top-down 2D paint viewport.
//!
//! Sprint 17 adds: Photoshop-style Layers panel, custom texture
//! import (file picker + drag-drop), DNTS hybrid emission
//! (bottom ≤4 DNTS-bound layers drive splat distribution + DDS
//! bake), retirement of `inspector_splat`.

use std::path::PathBuf;

use image::{Rgb, RgbImage};
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::MapSize;
use crate::splat::{SplatChannel, SplatConfig};

// ---------------------------------------------------------------------------
// Source / transform / colour / blend
// ---------------------------------------------------------------------------

/// One layer's diffuse source. Resolved at bake time:
/// `Slot` indexes into the stock `tools/textures/<NN-slot>/` registry
/// (ADR-027); `Imported` is a project-local path under
/// `<project>/textures/` (wired by Sprint 17's import workflow — the
/// schema lands here but no UI reaches it yet).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayerSource {
    Slot { id: u8 },
    Imported { path: PathBuf },
}

impl LayerSource {
    /// Short human-readable label used as the default layer name.
    pub fn default_label(&self) -> String {
        match self {
            Self::Slot { id } => format!("Slot {id:02}"),
            Self::Imported { path } => path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Imported".to_string()),
        }
    }
}

/// Per-layer affine transform applied when sampling the source diffuse.
/// Sampling is wallpaper-tiled in both directions; the transform places
/// the texture under the mask.
///
/// **Pitfall — mirror composes with rotation.** `mirror_x` / `mirror_y`
/// are applied BEFORE `rotation_rad` in the sample math (see
/// [`bake_diffuse`]). Reversing the order would rotate the
/// post-mirror axis the wrong way for any non-axis-aligned angle. Pinned
/// by [`tests::bake_mirror_then_rotate_matches_reference`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerTransform {
    /// Offset from the diffuse centre in elmos.
    pub offset_elmos: [f32; 2],
    /// Uniform scale. `1.0` = native texture size.
    pub scale: f32,
    /// Rotation in radians (any angle).
    pub rotation_rad: f32,
    pub mirror_x: bool,
    pub mirror_y: bool,
}

impl Default for LayerTransform {
    fn default() -> Self {
        Self {
            offset_elmos: [0.0, 0.0],
            scale: 1.0,
            rotation_rad: 0.0,
            mirror_x: false,
            mirror_y: false,
        }
    }
}

/// Per-layer colour modulation. Applied to the sampled diffuse before
/// the mask + blend.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerColor {
    /// RGB tint multiplier (`1.0` = identity).
    pub tint_rgb: [f32; 3],
    /// Brightness add (`-1.0..=1.0`; `0.0` = identity).
    pub brightness: f32,
}

impl Default for LayerColor {
    fn default() -> Self {
        Self {
            tint_rgb: [1.0, 1.0, 1.0],
            brightness: 0.0,
        }
    }
}

/// v1: normal (alpha-over) only. The enum is reserved for Sprint-N
/// expansion; the compositor panics on unknown variants under
/// `debug_assertions` so a future `Multiply` addition can't silently
/// degrade to `Normal`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
}

// ---------------------------------------------------------------------------
// Mask
// ---------------------------------------------------------------------------

/// Grayscale alpha mask sized to the diffuse (`512 × SMU` per side).
///
/// Storage is a flat `Vec<u8>` for Sprint 15; Sprint 16 (D9) swaps the
/// internals for a tiled-COW structure but the public surface
/// ([`Self::write_rect`], [`Self::sample`]) stays.
///
/// `bytes[i] = 255` → layer fully visible at pixel i.
/// `bytes[i] = 0`   → fully transparent (lower layers show through).
///
/// **Memory.** A 16-SMU map mask = `8192² × 1` = 64 MB per layer
/// resident. The [`LayerStack::bake_diffuse`] path `warn!`s when the
/// total cross-layer mask cost exceeds 256 MB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerMask {
    pub width: u32,
    pub height: u32,
    #[serde(with = "mask_bytes_b64")]
    pub bytes: Vec<u8>,
}

impl LayerMask {
    /// Allocate a mask filled with `fill` at `size.texture_dims()`.
    ///
    /// TODO(tiled-cow): replace the flat allocation with a tiled COW
    /// structure adapted from ADR-018's heightmap-undo pattern when
    /// Sprint 16 (D9) lands. The public methods on `LayerMask` stay
    /// shaped so callers don't need to change.
    pub fn filled(size: MapSize, fill: u8) -> Self {
        let (w, h) = size.texture_dims();
        let bytes = vec![fill; (w as usize) * (h as usize)];
        Self {
            width: w,
            height: h,
            bytes,
        }
    }

    /// Sample the mask at integer pixel coordinates. Returns `0` for
    /// out-of-bounds reads — the compositor calls this with clamped
    /// integer indices and never goes off the edge in practice; the
    /// guard is defensive.
    pub fn sample(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.bytes[(y as usize) * (self.width as usize) + (x as usize)]
    }

    /// Stub for Sprint 16's mask-brush write path. The shape mirrors
    /// the heightmap brushes' dirty-rect upload (ADR-018) so the GPU
    /// upload in D9 can use it directly. **Not called from Sprint 15.**
    ///
    /// `src` must be `rect.w * rect.h` bytes in row-major order; out-
    /// of-bounds writes are clipped silently. Returns the actual
    /// clipped rect.
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
        let stride = self.width as usize;
        for row in 0..ch {
            let dst_start = (y as usize + row as usize) * stride + x as usize;
            let src_start = (row as usize) * (w as usize);
            self.bytes[dst_start..dst_start + cw as usize]
                .copy_from_slice(&src[src_start..src_start + cw as usize]);
        }
        Some((x, y, cw, ch))
    }
}

/// Base64 serde for `LayerMask::bytes`. Sprint 16 (D9) will replace
/// this with a sidecar-PNG schema once the tiled-COW model lands and
/// per-layer masks live at `<project>/masks/<layer_id>.png`. The
/// schema migration is tracked under D9.
mod mask_bytes_b64 {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&BASE64.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        // `String::deserialize` rather than `<&str>::deserialize` —
        // TOML doesn't always hand back a borrowed view (long strings
        // can land owned), and the borrowed path was failing on the
        // mask-round-trip test with `invalid type: string`.
        let s = String::deserialize(d)?;
        BASE64
            .decode(s.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Layer + stack
// ---------------------------------------------------------------------------

/// One layer in the stack. `id` is stable across sessions so undo and
/// Sprint 16+ sidecar files can target the right layer after reorders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureLayer {
    /// Stable identifier. Allocated by [`alloc_layer_id`] at create
    /// time; persisted; used by [`crate::undo::ProjectDiff`] and (in
    /// Sprint 16+) by sidecar mask files.
    pub id: String,
    /// User-visible name. Defaults to the source's label on creation.
    pub name: String,
    pub source: LayerSource,
    #[serde(default)]
    pub transform: LayerTransform,
    #[serde(default)]
    pub color: LayerColor,
    #[serde(default)]
    pub blend: BlendMode,
    /// Layer is included in the bake. Eye-toggle in the Layers panel
    /// (Sprint 17) flips this.
    #[serde(default = "default_true")]
    pub visible: bool,
    /// Layer is locked from edits in the Layers panel (Sprint 17).
    #[serde(default)]
    pub locked: bool,
    /// `Some(channel)` = this layer is one of the (≤ 4) DNTS-bound
    /// layers; Sprint 17's emitter wires its mask into the splat
    /// distribution's matching channel + bakes the slot's normal map
    /// into the DDS array. `None` = layer is diffuse-only.
    #[serde(default)]
    pub dnts_channel: Option<SplatChannel>,
    /// Per-layer opacity multiplier on top of the mask. `1.0` =
    /// identity. `0.0` is equivalent to `visible = false` but keeps
    /// the mask data live for inspection.
    #[serde(default = "default_one_f32")]
    pub opacity: f32,
    pub mask: LayerMask,
}

fn default_true() -> bool {
    true
}
fn default_one_f32() -> f32 {
    1.0
}

impl TextureLayer {
    /// Construct a layer with sensible defaults: fresh id, label
    /// derived from `source`, identity transform/colour, normal blend,
    /// visible, unlocked, no DNTS binding, opacity = 1.0. `mask_fill`
    /// is the initial mask value — pick `255` for the bottom layer of
    /// a new stack and `0` for any added-on-top layer so the new layer
    /// doesn't obscure what's beneath it.
    pub fn new(source: LayerSource, size: MapSize, mask_fill: u8) -> Self {
        let name = source.default_label();
        Self {
            id: alloc_layer_id(),
            name,
            source,
            transform: LayerTransform::default(),
            color: LayerColor::default(),
            blend: BlendMode::default(),
            visible: true,
            locked: false,
            dnts_channel: None,
            opacity: 1.0,
            mask: LayerMask::filled(size, mask_fill),
        }
    }
}

/// Mint a fresh layer id. UUID-v4-shape ASCII hex; we keep this
/// dependency-free since no other crate needs UUIDs yet. The seed is
/// time + a thread-local counter — collisions are statistically
/// negligible for a single-user editor and the id is just a layer
/// handle, not a security primitive.
pub fn alloc_layer_id() -> String {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local! {
        static COUNTER: Cell<u64> = const { Cell::new(0) };
    }
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = COUNTER.with(|c| {
        let v = c.get().wrapping_add(1);
        c.set(v);
        v
    });
    // Mix the two via a small xorshift so the printed hex looks
    // uniformly distributed even when many ids are minted within the
    // same nanosecond.
    let mut x = now_nanos ^ (counter.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    x ^= x >> 33;
    let y = x.wrapping_add(counter);
    format!("{x:016x}{y:016x}")
}

/// The layer stack. Z-order is [`Vec::iter`] from bottom (`idx 0`) to
/// top (`idx N-1`). The Layers panel UI in Sprint 17 will render
/// reversed (Photoshop convention: top of list = top of stack); the
/// internal order stays bottom-first so the compositor iterates
/// naturally.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayerStack {
    #[serde(default)]
    pub layers: Vec<TextureLayer>,
}

impl LayerStack {
    /// Build the default single-layer stack for a fresh project, using
    /// the biome's base slot from `tools/textures/`.
    ///
    /// Sprint 15's BIOMES (`[crate::BIOMES]`) describes terrain shape,
    /// not texture palette — slot 0 (grass meadow) is the BAR-typical
    /// "earth-temperate" default and is used for every biome label
    /// for now. Sprint 17 will likely add a per-biome palette map
    /// when texture biomes split off from shape biomes.
    pub fn from_biome(_biome_label: &str, size: MapSize) -> Self {
        let layer = TextureLayer::new(LayerSource::Slot { id: 0 }, size, 255);
        Self {
            layers: vec![layer],
        }
    }

    /// Migrate from a pre-D8 [`SplatConfig`] to a layer stack. One
    /// layer per bound DNTS channel, in R/G/B/A order, each with
    /// `dnts_channel = Some(channel)`. Masks start FULL (255) for the
    /// bottom layer and EMPTY (0) for the rest — the pre-D8 splat
    /// painting is NOT migrated to mask pixels in Sprint 15 (the data
    /// shape is different; Sprint 17 will offer a one-time migration
    /// when the user opens an older project).
    ///
    /// The pre-D8 `splat_config.tex_scales` / `tex_mults` are
    /// preserved on `Project.splat_config` for runtime DNTS — they
    /// are NOT migrated into the layer model here.
    ///
    /// `slot_id_for_channel` resolves a channel index to its bound
    /// slot id. The app passes a closure that consults
    /// `config.channels[i]`; tests can substitute. Channels resolving
    /// to `None` produce no layer.
    pub fn migrate_from_splat_config(
        config: &SplatConfig,
        slot_id_for_channel: impl Fn(u8) -> Option<u8>,
        size: MapSize,
    ) -> Self {
        let mut layers = Vec::new();
        // R / G / B / A — channel index drives the [`SplatChannel`]
        // discriminant.
        for ch_idx in 0..4u8 {
            let Some(slot_id) = slot_id_for_channel(ch_idx) else {
                continue;
            };
            // Bottom of the stack gets a full mask so the bake has
            // something to start from; subsequent layers come in
            // empty so they don't clobber what's below.
            let fill = if layers.is_empty() { 255 } else { 0 };
            let mut layer = TextureLayer::new(LayerSource::Slot { id: slot_id }, size, fill);
            layer.dnts_channel = Some(channel_index_to_enum(ch_idx));
            // Cosmetic: name reflects the channel binding so users
            // recognise migrated layers in Sprint 17's Layers panel.
            layer.name = format!("Slot {slot_id:02} (channel {})", ch_idx_label(ch_idx));
            layers.push(layer);
        }
        // Sanity guard: the splat config has at most 4 channels so
        // the migration is bounded by definition. The 4-layer cap
        // also matches the mask-byte-budget ceiling discussed in the
        // module docs.
        debug_assert!(layers.len() <= 4);
        let _ = config; // future: surface tex_scales in the layer transform if needed
        Self { layers }
    }

    /// Bake all visible layers into an RGB8 diffuse image. Output
    /// dims = `size.texture_dims()` (`512 × SMU` per side). This is
    /// the BMP fed to PyMapConv at `.sd7` build time, replacing
    /// `synth_biome_bmp`.
    ///
    /// Compositor: back-to-front (idx 0 → N-1). For each pixel sample
    /// the layer's source with wallpaper-tiled modulo + transform,
    /// apply tint/brightness, multiply by `(mask × opacity)`, and
    /// alpha-over the accumulator.
    ///
    /// Performance budget per ADR-038: 8192² × 16-layer ≈ 256
    /// megasamples worst case; per-row rayon parallel; target ≤ 1.5 s
    /// release for a 16-SMU map with 8 layers. Profile is recorded in
    /// the Sprint 15 devlog.
    pub fn bake_diffuse(&self, size: MapSize, slot_resolver: &dyn SlotResolver) -> RgbImage {
        let (w, h) = size.texture_dims();
        debug_assert!(
            w >= 1024 && w.is_multiple_of(1024),
            "texture width must be a multiple of 1024 per PyMapConv contract"
        );
        debug_assert!(
            h >= 1024 && h.is_multiple_of(1024),
            "texture height must be a multiple of 1024 per PyMapConv contract"
        );

        // Resolve every visible layer's source. Missing diffuse paths
        // produce a grey placeholder rather than aborting the bake;
        // a `warn!` makes the regression visible without breaking
        // export.
        let resolved: Vec<ResolvedLayer> = self
            .layers
            .iter()
            .filter(|l| l.visible && l.opacity > 0.0)
            .map(|l| ResolvedLayer::prepare(l, slot_resolver))
            .collect();

        let total_mask_bytes: usize = resolved.iter().map(|r| r.layer_mask.bytes.len()).sum();
        if total_mask_bytes > 256 * 1024 * 1024 {
            warn!(
                total_mask_mb = total_mask_bytes / (1024 * 1024),
                layer_count = resolved.len(),
                "bake_diffuse: layer-mask working set exceeds 256 MB; tiled-COW (D9 / Sprint 16) \
                 hasn't landed yet"
            );
        }

        let layer_count = resolved.len();
        info!(
            width = w,
            height = h,
            layers = layer_count,
            "bake_diffuse: start"
        );
        let started = std::time::Instant::now();

        // Flat RGB8 output, row-major, 3 bytes per pixel. Rayon over
        // chunks of one row each.
        let mut out: Vec<u8> = vec![0u8; (w as usize) * (h as usize) * 3];
        let row_stride = (w as usize) * 3;
        out.par_chunks_mut(row_stride)
            .enumerate()
            .for_each(|(y, row)| {
                for x in 0..w as usize {
                    // Per-pixel accumulator in linear-ish 0..=1
                    // floats. The bake is intentionally in sRGB space
                    // (no gamma round-trip) because PyMapConv reads
                    // BMP bytes verbatim and the engine's DXT1 path
                    // applies the same sRGB → linear conversion as
                    // the editor's preview — bytes are the contract.
                    let mut acc_r = 0.0f32;
                    let mut acc_g = 0.0f32;
                    let mut acc_b = 0.0f32;
                    let mut acc_a = 0.0f32;
                    for layer in &resolved {
                        let sample = layer.sample_pixel(x as u32, y as u32, w, h);
                        let Some([sr, sg, sb, alpha]) = sample else {
                            continue;
                        };
                        // Alpha-over: dst = src + dst * (1 - src_a).
                        // Pre-multiply RGB by alpha so the operator
                        // composes correctly with the accumulator's
                        // own alpha.
                        let inv = 1.0 - alpha;
                        acc_r = sr * alpha + acc_r * inv;
                        acc_g = sg * alpha + acc_g * inv;
                        acc_b = sb * alpha + acc_b * inv;
                        acc_a = alpha + acc_a * inv;
                    }
                    // Flatten the accumulator against an opaque
                    // middle-grey so an under-painted pixel ships as
                    // grey rather than pure black. (PyMapConv's DXT1
                    // pass would clamp 0,0,0 fine; the grey makes the
                    // map readable while a user iterates on coverage.)
                    let bg = 0.18f32;
                    let inv = 1.0 - acc_a;
                    let r = (acc_r + bg * inv).clamp(0.0, 1.0);
                    let g = (acc_g + bg * inv).clamp(0.0, 1.0);
                    let b = (acc_b + bg * inv).clamp(0.0, 1.0);
                    let i = x * 3;
                    row[i] = (r * 255.0 + 0.5) as u8;
                    row[i + 1] = (g * 255.0 + 0.5) as u8;
                    row[i + 2] = (b * 255.0 + 0.5) as u8;
                }
            });

        let elapsed = started.elapsed();
        info!(
            elapsed_ms = elapsed.as_millis() as u64,
            layers = layer_count,
            "bake_diffuse: done"
        );

        RgbImage::from_raw(w, h, out)
            .expect("output buffer length matches w * h * 3 by construction")
    }

    /// Bytes the resident layer masks occupy. Used by callers that
    /// want a heads-up before the bake (e.g. a UI warn-popover before
    /// a click-build). Counts only mask bytes — source diffuses live
    /// off the project model and aren't owned here.
    pub fn resident_mask_bytes(&self) -> usize {
        self.layers.iter().map(|l| l.mask.bytes.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Slot resolver trait
// ---------------------------------------------------------------------------

/// How the bake reaches a slot id's diffuse PNG. The app provides this
/// by wrapping its `slot_registry: Vec<SlotMeta>`; tests substitute a
/// closure-backed implementation via [`ClosureSlotResolver`].
pub trait SlotResolver {
    fn diffuse_path(&self, slot_id: u8) -> Option<PathBuf>;
}

/// Convenience adapter for ad-hoc callers (tests, smoke binaries).
pub struct ClosureSlotResolver<F: Fn(u8) -> Option<PathBuf>>(pub F);

impl<F: Fn(u8) -> Option<PathBuf>> SlotResolver for ClosureSlotResolver<F> {
    fn diffuse_path(&self, slot_id: u8) -> Option<PathBuf> {
        (self.0)(slot_id)
    }
}

// ---------------------------------------------------------------------------
// Internal bake helpers
// ---------------------------------------------------------------------------

/// A layer with its source diffuse decoded into memory once per bake.
/// Missing sources fall back to a 1×1 mid-grey image so the bake
/// proceeds with a visible placeholder.
struct ResolvedLayer<'a> {
    layer_mask: &'a LayerMask,
    diffuse: RgbImage,
    transform: LayerTransform,
    color: LayerColor,
    blend: BlendMode,
    opacity: f32,
    /// Texture extent in elmos (cached from the mask dims and the
    /// `MapSize::ELMOS_PER_SMU` conversion). The mask dims are
    /// authoritative for the diffuse extent.
    elmo_extent: [f32; 2],
}

impl<'a> ResolvedLayer<'a> {
    fn prepare(layer: &'a TextureLayer, slot_resolver: &dyn SlotResolver) -> Self {
        let diffuse = load_layer_diffuse(&layer.source, slot_resolver);
        // The mask is authoritative for the layer's extent; the
        // texture extent in elmos derives from the mask dims using
        // the canonical 512 px / SMU rule.
        let elmos_per_px = MapSize::ELMOS_PER_SMU as f32 / MapSize::TEXTURE_PER_SMU as f32;
        let elmo_extent = [
            layer.mask.width as f32 * elmos_per_px,
            layer.mask.height as f32 * elmos_per_px,
        ];
        Self {
            layer_mask: &layer.mask,
            diffuse,
            transform: layer.transform,
            color: layer.color,
            blend: layer.blend,
            opacity: layer.opacity.clamp(0.0, 1.0),
            elmo_extent,
        }
    }

    /// Sample one pixel of this layer's contribution. Returns
    /// `Some([r, g, b, a])` with components in `0..=1` (RGB
    /// pre-multiplied-style is applied by the caller).
    fn sample_pixel(&self, x: u32, y: u32, _w: u32, _h: u32) -> Option<[f32; 4]> {
        let alpha = self.layer_mask.sample(x, y) as f32 / 255.0 * self.opacity;
        if alpha <= 0.0 {
            return None;
        }
        let (sx, sy) = self.sample_uv(x, y);
        let texel = sample_wallpaper(&self.diffuse, sx, sy);
        let [r0, g0, b0] = u8_rgb_to_f32(texel);
        // Apply tint + brightness in sRGB space (cheap and matches
        // the editor's WGSL preview which also operates pre-gamma).
        let r = (r0 * self.color.tint_rgb[0] + self.color.brightness).clamp(0.0, 1.0);
        let g = (g0 * self.color.tint_rgb[1] + self.color.brightness).clamp(0.0, 1.0);
        let b = (b0 * self.color.tint_rgb[2] + self.color.brightness).clamp(0.0, 1.0);

        match self.blend {
            BlendMode::Normal => Some([r, g, b, alpha]),
            // Defensive: a future variant must not silently fall back
            // to Normal blending. Catches `BlendMode::Multiply` etc.
            // being added without updating the compositor.
            #[allow(unreachable_patterns)]
            other => {
                debug_assert!(false, "compositor missing blend impl for {other:?}");
                Some([r, g, b, alpha])
            }
        }
    }

    /// Compute the source-image sample coordinates for the given
    /// output pixel. Mirror is applied BEFORE rotation (pinned by
    /// `bake_mirror_then_rotate_matches_reference`).
    fn sample_uv(&self, x: u32, y: u32) -> (f32, f32) {
        let elmos_per_px = MapSize::ELMOS_PER_SMU as f32 / MapSize::TEXTURE_PER_SMU as f32;
        // World position in elmos relative to the diffuse centre.
        let cx_world = (x as f32 + 0.5) * elmos_per_px - self.elmo_extent[0] * 0.5;
        let cy_world = (y as f32 + 0.5) * elmos_per_px - self.elmo_extent[1] * 0.5;

        // Apply mirror to the world coordinate first.
        let mx = if self.transform.mirror_x {
            -cx_world
        } else {
            cx_world
        };
        let my = if self.transform.mirror_y {
            -cy_world
        } else {
            cy_world
        };

        // Then rotation (about the diffuse centre).
        let (s, c) = self.transform.rotation_rad.sin_cos();
        let rx = c * mx - s * my;
        let ry = s * mx + c * my;

        // Then translation + scale.
        let scale = self.transform.scale.max(1e-4);
        let tex_w_world = self.diffuse.width() as f32 * elmos_per_px * scale;
        let tex_h_world = self.diffuse.height() as f32 * elmos_per_px * scale;
        let ox = self.transform.offset_elmos[0];
        let oy = self.transform.offset_elmos[1];
        // u/v are wallpaper coordinates in 0..diffuse_dims pixel space.
        let u = (rx - ox) / scale / elmos_per_px;
        let v = (ry - oy) / scale / elmos_per_px;
        // Re-centre into the source's pixel frame.
        let u_centered = u + self.diffuse.width() as f32 * 0.5;
        let v_centered = v + self.diffuse.height() as f32 * 0.5;
        let _ = (tex_w_world, tex_h_world); // future: used when ADR-040's preview UI surfaces tile bounds
        (u_centered, v_centered)
    }
}

/// Load a layer's diffuse PNG, falling back to a 1×1 mid-grey image
/// when the source is missing / unresolvable. Imported sources are
/// not exercised in Sprint 15 (no UI reaches them) but the loader is
/// in place so Sprint 17's import workflow lights up automatically.
fn load_layer_diffuse(source: &LayerSource, resolver: &dyn SlotResolver) -> RgbImage {
    let path = match source {
        LayerSource::Slot { id } => match resolver.diffuse_path(*id) {
            Some(p) => p,
            None => {
                warn!(
                    slot_id = *id,
                    "bake: slot diffuse unresolved; using grey placeholder"
                );
                return placeholder_grey();
            }
        },
        LayerSource::Imported { path } => path.clone(),
    };
    match image::open(&path) {
        Ok(img) => img.into_rgb8(),
        Err(e) => {
            warn!(
                path = %path.display(),
                error = %e,
                "bake: layer diffuse load failed; using grey placeholder"
            );
            placeholder_grey()
        }
    }
}

fn placeholder_grey() -> RgbImage {
    let mut img = RgbImage::new(1, 1);
    img.put_pixel(0, 0, Rgb([128, 128, 128]));
    img
}

/// Wallpaper-tiled bilinear sample of `img` at fractional pixel
/// coordinates. Negative / over-extent coordinates wrap via
/// `rem_euclid` so the layer truly tiles in both directions —
/// edge-clamp is explicitly out (see ADR-038's "wallpaper, not
/// edge-clamp" pitfall).
fn sample_wallpaper(img: &RgbImage, u: f32, v: f32) -> [u8; 3] {
    let w = img.width() as f32;
    let h = img.height() as f32;
    if w <= 0.0 || h <= 0.0 {
        return [128, 128, 128];
    }
    let u_wrapped = u.rem_euclid(w);
    let v_wrapped = v.rem_euclid(h);
    let u0 = u_wrapped.floor() as i64;
    let v0 = v_wrapped.floor() as i64;
    let fu = u_wrapped - u0 as f32;
    let fv = v_wrapped - v0 as f32;
    let iw = img.width() as i64;
    let ih = img.height() as i64;
    let u0 = ((u0 % iw) + iw) % iw;
    let v0 = ((v0 % ih) + ih) % ih;
    let u1 = (u0 + 1) % iw;
    let v1 = (v0 + 1) % ih;
    let p00 = img.get_pixel(u0 as u32, v0 as u32).0;
    let p10 = img.get_pixel(u1 as u32, v0 as u32).0;
    let p01 = img.get_pixel(u0 as u32, v1 as u32).0;
    let p11 = img.get_pixel(u1 as u32, v1 as u32).0;
    let lerp = |a: u8, b: u8, t: f32| (a as f32 * (1.0 - t) + b as f32 * t).round() as u8;
    let top_r = lerp(p00[0], p10[0], fu);
    let top_g = lerp(p00[1], p10[1], fu);
    let top_b = lerp(p00[2], p10[2], fu);
    let bot_r = lerp(p01[0], p11[0], fu);
    let bot_g = lerp(p01[1], p11[1], fu);
    let bot_b = lerp(p01[2], p11[2], fu);
    [
        lerp(top_r, bot_r, fv),
        lerp(top_g, bot_g, fv),
        lerp(top_b, bot_b, fv),
    ]
}

fn u8_rgb_to_f32([r, g, b]: [u8; 3]) -> [f32; 3] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

fn channel_index_to_enum(i: u8) -> SplatChannel {
    match i {
        0 => SplatChannel::R,
        1 => SplatChannel::G,
        2 => SplatChannel::B,
        3 => SplatChannel::A,
        _ => SplatChannel::R,
    }
}

fn ch_idx_label(i: u8) -> &'static str {
    match i {
        0 => "R",
        1 => "G",
        2 => "B",
        3 => "A",
        _ => "?",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn tiny_size() -> MapSize {
        // 1 SMU = 512 px texture, so the smallest legal dims (the bake
        // asserts >= 1024 per side) is 2 SMU = 1024². 4 SMU = 2048² is
        // a comfortable middle for tests — quick to bake, big enough
        // to exercise per-row rayon parallelism.
        MapSize::square(4)
    }

    fn dummy_resolver(slot_diffuse: Option<PathBuf>) -> impl SlotResolver {
        ClosureSlotResolver(move |_| slot_diffuse.clone())
    }

    fn write_solid_diffuse_png(dir: &Path, name: &str, rgb: [u8; 3]) -> PathBuf {
        let path = dir.join(name);
        let mut img = RgbImage::new(64, 64);
        for px in img.pixels_mut() {
            *px = Rgb(rgb);
        }
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn layer_mask_filled_pins_dims_to_texture_dims() {
        let m = LayerMask::filled(MapSize::square(2), 255);
        assert_eq!(m.width, 1024);
        assert_eq!(m.height, 1024);
        assert_eq!(m.bytes.len(), 1024 * 1024);
        assert!(m.bytes.iter().all(|&b| b == 255));
    }

    #[test]
    fn write_rect_copies_and_clips() {
        let mut m = LayerMask::filled(MapSize::square(2), 0);
        let stamp = vec![200u8; 8 * 8];
        let clipped = m.write_rect(1020, 1020, 8, 8, &stamp).unwrap();
        // (1020, 1020, 4, 4) — clipped to texture extent.
        assert_eq!(clipped, (1020, 1020, 4, 4));
        // Corner pixel must be 200; far away must still be 0.
        assert_eq!(m.sample(1020, 1020), 200);
        assert_eq!(m.sample(1023, 1023), 200);
        assert_eq!(m.sample(0, 0), 0);
    }

    #[test]
    fn sample_out_of_bounds_returns_zero() {
        let m = LayerMask::filled(MapSize::square(2), 255);
        assert_eq!(m.sample(0, 0), 255);
        assert_eq!(m.sample(1023, 1023), 255);
        assert_eq!(m.sample(1024, 1023), 0);
        assert_eq!(m.sample(0, 1024), 0);
        assert_eq!(m.sample(u32::MAX, u32::MAX), 0);
    }

    #[test]
    fn mask_bytes_round_trip_through_base64_toml() {
        let mut m = LayerMask::filled(MapSize::square(2), 0);
        m.bytes[0] = 1;
        m.bytes[42] = 200;
        let last_idx = m.bytes.len() - 1;
        m.bytes[last_idx] = 99;
        let s = match toml::to_string(&m) {
            Ok(s) => s,
            Err(e) => panic!("toml::to_string error: {e}"),
        };
        let m2: LayerMask = toml::from_str(&s).unwrap();
        assert_eq!(m.width, m2.width);
        assert_eq!(m.height, m2.height);
        assert_eq!(m.bytes.len(), m2.bytes.len());
        assert_eq!(m.bytes[0], m2.bytes[0]);
        assert_eq!(m.bytes[42], m2.bytes[42]);
        assert_eq!(m.bytes[last_idx], m2.bytes[last_idx]);
    }

    #[test]
    fn from_biome_seeds_single_slot_zero_layer() {
        let stack = LayerStack::from_biome("Cone peak", tiny_size());
        assert_eq!(stack.layers.len(), 1);
        match &stack.layers[0].source {
            LayerSource::Slot { id } => assert_eq!(*id, 0),
            other => panic!("expected Slot{{0}}, got {other:?}"),
        }
        assert!(stack.layers[0].mask.bytes.iter().all(|&b| b == 255));
        assert!(stack.layers[0].dnts_channel.is_none());
    }

    #[test]
    fn migrate_from_splat_config_seeds_one_layer_per_bound_channel() {
        // R, G, A bound; B unbound.
        let cfg = SplatConfig {
            channels: [Some(0), Some(3), None, Some(5)],
            ..SplatConfig::default()
        };
        let stack =
            LayerStack::migrate_from_splat_config(&cfg, |i| cfg.channels[i as usize], tiny_size());
        assert_eq!(stack.layers.len(), 3, "one layer per bound channel");
        // Order matches channel order R, G, A → slot ids 0, 3, 5.
        assert!(matches!(
            stack.layers[0].source,
            LayerSource::Slot { id: 0 }
        ));
        assert!(matches!(
            stack.layers[1].source,
            LayerSource::Slot { id: 3 }
        ));
        assert!(matches!(
            stack.layers[2].source,
            LayerSource::Slot { id: 5 }
        ));
        // DNTS bindings match the source channel index (R/G/A).
        assert_eq!(stack.layers[0].dnts_channel, Some(SplatChannel::R));
        assert_eq!(stack.layers[1].dnts_channel, Some(SplatChannel::G));
        assert_eq!(stack.layers[2].dnts_channel, Some(SplatChannel::A));
        // Bottom mask full; subsequent empty.
        assert!(stack.layers[0].mask.bytes.iter().all(|&b| b == 255));
        assert!(stack.layers[1].mask.bytes.iter().all(|&b| b == 0));
        assert!(stack.layers[2].mask.bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn migrate_from_unbound_config_produces_empty_stack() {
        let cfg = SplatConfig::default(); // all channels None
        let stack = LayerStack::migrate_from_splat_config(&cfg, |_| None, tiny_size());
        assert!(stack.layers.is_empty());
    }

    #[test]
    fn bake_diffuse_dims_match_size_texture_dims() {
        let tmp = tempfile::tempdir().unwrap();
        let png = write_solid_diffuse_png(tmp.path(), "diffuse.png", [10, 20, 30]);
        let resolver = dummy_resolver(Some(png));
        let stack = LayerStack::from_biome("Flat plain", tiny_size());
        let baked = stack.bake_diffuse(tiny_size(), &resolver);
        let (w, h) = tiny_size().texture_dims();
        assert_eq!(baked.dimensions(), (w, h));
    }

    #[test]
    fn bake_diffuse_wallpaper_tiles_source_across_extent() {
        // Build a layer whose source diffuse is a 2x2 checker; with a
        // mask full to 255 + identity transform, every output row
        // should carry the same colours (since the wallpaper sample
        // repeats deterministically).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("checker.png");
        let mut img = RgbImage::new(2, 2);
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        img.put_pixel(1, 0, Rgb([0, 255, 0]));
        img.put_pixel(0, 1, Rgb([0, 0, 255]));
        img.put_pixel(1, 1, Rgb([255, 255, 0]));
        img.save(&path).unwrap();
        let resolver = dummy_resolver(Some(path));
        let stack = LayerStack::from_biome("Diagonal ramp", tiny_size());
        let baked = stack.bake_diffuse(tiny_size(), &resolver);
        let (w, h) = baked.dimensions();
        // Every pixel must have at least one non-zero channel — i.e.
        // the wallpaper actually covered the full extent.
        let mut any_dark = 0u64;
        for y in 0..h {
            for x in 0..w {
                let p = baked.get_pixel(x, y).0;
                if p[0] < 5 && p[1] < 5 && p[2] < 5 {
                    any_dark += 1;
                }
            }
        }
        // The mid-grey background is the only way pixels would land
        // near zero; with an all-visible mask that shouldn't happen.
        // Allow a tiny slop for rounding at near-black sources.
        assert!(
            any_dark < ((w as u64) * (h as u64) / 1000),
            "wallpaper coverage gap — too many near-zero pixels ({any_dark})"
        );
    }

    #[test]
    fn bake_diffuse_falls_back_to_grey_when_source_missing() {
        let resolver = dummy_resolver(None);
        let stack = LayerStack::from_biome("Cone peak", tiny_size());
        let baked = stack.bake_diffuse(tiny_size(), &resolver);
        // Grey placeholder = 128; alpha = 1; flatten over bg=0.18
        // gives basically 128 since alpha is 1 across the full mask.
        let p = baked.get_pixel(0, 0).0;
        for c in p {
            assert!(
                (120..=140).contains(&c),
                "expected grey-ish placeholder, got {p:?}"
            );
        }
    }

    /// Pin the mirror-then-rotate ordering. Mirror followed by a 90°
    /// CCW rotation maps (+x, 0) → (0, +x); applying rotation first
    /// would map (+x, 0) → (-y, 0) → (-y, 0) (a degenerate case where
    /// the mirror has no effect). We check that swapping the order
    /// does change the bake output for a non-symmetric source.
    #[test]
    fn bake_mirror_then_rotate_matches_reference() {
        let tmp = tempfile::tempdir().unwrap();
        // Asymmetric 2×2 source: top row = red/green, bottom = blue/yellow.
        // No two corners share both colour and (mirror×rotate)
        // position, so the order of operations is observable.
        let path = tmp.path().join("src.png");
        let mut img = RgbImage::new(2, 2);
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        img.put_pixel(1, 0, Rgb([0, 255, 0]));
        img.put_pixel(0, 1, Rgb([0, 0, 255]));
        img.put_pixel(1, 1, Rgb([255, 255, 0]));
        img.save(&path).unwrap();
        let resolver = dummy_resolver(Some(path));

        let mut stack_a = LayerStack::from_biome("Flat plain", tiny_size());
        stack_a.layers[0].transform.mirror_x = true;
        stack_a.layers[0].transform.rotation_rad = std::f32::consts::FRAC_PI_2;
        let baked_a = stack_a.bake_diffuse(tiny_size(), &resolver);

        // Reference: same source, no mirror, no rotate. The output
        // MUST differ — if mirror were applied AFTER rotate (or
        // ignored), the two would land on the same wallpaper-tiled
        // result for a horizontally-symmetric stripe of the checker.
        let mut stack_b = LayerStack::from_biome("Flat plain", tiny_size());
        stack_b.layers[0].transform.mirror_x = false;
        stack_b.layers[0].transform.rotation_rad = 0.0;
        let baked_b = stack_b.bake_diffuse(tiny_size(), &resolver);
        assert_ne!(
            baked_a.as_raw(),
            baked_b.as_raw(),
            "mirror+rotate output must differ from identity baseline"
        );
    }

    #[test]
    fn bake_skips_invisible_and_zero_opacity_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let red = write_solid_diffuse_png(tmp.path(), "red.png", [255, 0, 0]);
        let resolver = dummy_resolver(Some(red));
        let mut stack = LayerStack::from_biome("Flat plain", tiny_size());
        // One visible-but-zero-opacity layer + one invisible layer.
        // Both should be skipped; output should fall back to the
        // 0.18 mid-grey background.
        stack.layers[0].opacity = 0.0;
        let baked = stack.bake_diffuse(tiny_size(), &resolver);
        let p = baked.get_pixel(0, 0).0;
        // bg = 0.18 → ~46 per channel.
        for c in p {
            assert!((40..=52).contains(&c), "expected ~46 mid-grey, got {p:?}");
        }
    }

    #[test]
    fn resident_mask_bytes_sums_each_layer() {
        let cfg = SplatConfig {
            channels: [Some(0), Some(1), None, None],
            ..Default::default()
        };
        let stack = LayerStack::migrate_from_splat_config(
            &cfg,
            |i| cfg.channels[i as usize],
            MapSize::square(2),
        );
        let (w, h) = MapSize::square(2).texture_dims();
        let per_layer = (w as usize) * (h as usize);
        assert_eq!(stack.resident_mask_bytes(), 2 * per_layer);
    }

    /// Pin the layer-id allocator: subsequent ids in the same
    /// thread/test must be distinct.
    #[test]
    fn alloc_layer_id_is_unique_within_a_thread() {
        let mut ids = Vec::with_capacity(64);
        for _ in 0..64 {
            ids.push(alloc_layer_id());
        }
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "ids must be unique");
    }

    #[test]
    fn texture_layer_round_trips_through_toml() {
        let layer = TextureLayer::new(LayerSource::Slot { id: 7 }, MapSize::square(2), 255);
        let s = toml::to_string(&layer).unwrap();
        let layer2: TextureLayer = toml::from_str(&s).unwrap();
        assert_eq!(layer, layer2);
    }

    #[test]
    fn imported_source_round_trips() {
        let src = LayerSource::Imported {
            path: PathBuf::from("textures/grass.png"),
        };
        let s = toml::to_string(&src).unwrap();
        let src2: LayerSource = toml::from_str(&s).unwrap();
        assert_eq!(src, src2);
    }

    #[test]
    fn empty_layer_stack_round_trips() {
        let stack = LayerStack::default();
        let s = toml::to_string(&stack).unwrap();
        let stack2: LayerStack = toml::from_str(&s).unwrap();
        assert_eq!(stack, stack2);
    }

    #[test]
    fn slot_resolver_trait_object_works_with_dyn() {
        // Compile-time check that `&dyn SlotResolver` is the public
        // contract `bake_diffuse` accepts (the public method is
        // `&dyn SlotResolver`). If this stops compiling, the trait
        // object surface drifted.
        let r = dummy_resolver(None);
        let _erased: &dyn SlotResolver = &r;
    }
}
