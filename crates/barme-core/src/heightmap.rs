//! 16-bit grayscale heightmap — Stage 0 lives in a flat `Vec<u16>`.
//!
//! The tiled, copy-on-write design (SRS §3.1) comes in Stage 1. For now we
//! just need a load/save path that respects the `64·N + 1` invariant
//! (PITFALL #1 — the most common silent corruption in this format).
//!
//! Storage is row-major, top-left origin, matching `image::ImageBuffer`.

use std::path::Path;

use anyhow::{Context, Result, bail};
use image::ImageReader;

use crate::MapSize;

#[derive(Debug, Clone)]
pub struct Heightmap {
    width: u32,
    height: u32,
    data: Vec<u16>,
}

impl Heightmap {
    pub fn new(width: u32, height: u32, data: Vec<u16>) -> Result<Self> {
        let expected = (width as usize) * (height as usize);
        if data.len() != expected {
            bail!(
                "heightmap pixel count mismatch: dims {}×{} = {} px but got {} samples",
                width,
                height,
                expected,
                data.len()
            );
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    pub fn data(&self) -> &[u16] {
        &self.data
    }

    /// Mutable access for brush kernels and other in-place transforms.
    /// Length always equals `width * height`; modify in row-major order.
    pub fn data_mut(&mut self) -> &mut [u16] {
        &mut self.data
    }

    /// Copy a sub-rect into a freshly-allocated buffer (row-major,
    /// `w * h` samples). Panics if the rect runs off the heightmap.
    /// Used by the undo system to snapshot pre-edit pixels.
    pub fn copy_rect(&self, x: u32, y: u32, w: u32, h: u32) -> Vec<u16> {
        assert!(
            x + w <= self.width && y + h <= self.height,
            "copy_rect {x},{y} {w}×{h} runs off {}×{} heightmap",
            self.width,
            self.height,
        );
        let mut out = Vec::with_capacity((w as usize) * (h as usize));
        for row in 0..h {
            let start = ((y + row) * self.width + x) as usize;
            let end = start + w as usize;
            out.extend_from_slice(&self.data[start..end]);
        }
        out
    }

    /// In-place swap of a sub-rect with `buf`. Both ends are populated:
    /// the rect's previous contents end up in `buf`, the new pixels end
    /// up in the heightmap. This is the atomic operation undo uses to
    /// flip between before / after states. Panics if dims mismatch.
    pub fn swap_rect(&mut self, x: u32, y: u32, w: u32, h: u32, buf: &mut [u16]) {
        let expected = (w as usize) * (h as usize);
        assert_eq!(buf.len(), expected, "swap_rect: buf size != w*h");
        assert!(
            x + w <= self.width && y + h <= self.height,
            "swap_rect {x},{y} {w}×{h} runs off {}×{} heightmap",
            self.width,
            self.height,
        );
        for row in 0..h {
            let dst_start = ((y + row) * self.width + x) as usize;
            let dst_row = &mut self.data[dst_start..dst_start + w as usize];
            let src_start = (row * w) as usize;
            let src_row = &mut buf[src_start..src_start + w as usize];
            dst_row.swap_with_slice(src_row);
        }
    }

    /// Verify dims match `MapSize::heightmap_dims()` — i.e. `64·N + 1`.
    /// Returning a typed error rather than `bail!` so the UI can show it.
    pub fn validate_against(&self, size: MapSize) -> Result<(), DimMismatch> {
        let expected = size.heightmap_dims();
        if (self.width, self.height) != expected {
            return Err(DimMismatch {
                expected,
                actual: (self.width, self.height),
            });
        }
        Ok(())
    }

    pub fn min_max(&self) -> (u16, u16) {
        self.data
            .iter()
            .copied()
            .fold((u16::MAX, u16::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
    }

    /// Decode a PNG. Accepts 16-bit grayscale natively; 8-bit grayscale is
    /// up-converted (each sample shifted into the high byte) so authors who
    /// hand us an 8-bit prototype still see something meaningful.
    pub fn load_png(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let img = ImageReader::open(path)
            .with_context(|| format!("opening {}", path.display()))?
            .decode()
            .with_context(|| format!("decoding {}", path.display()))?;

        let (w, h) = (img.width(), img.height());

        let data: Vec<u16> = match img {
            image::DynamicImage::ImageLuma16(buf) => buf.into_raw(),
            image::DynamicImage::ImageLuma8(buf) => buf
                .into_raw()
                .into_iter()
                .map(|v| (v as u16) << 8)
                .collect(),
            other => bail!(
                "heightmap PNG must be grayscale (Luma8 or Luma16); got {:?}",
                other.color()
            ),
        };

        Self::new(w, h, data)
    }

    /// Write a 16-bit grayscale PNG.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let buf: image::ImageBuffer<image::Luma<u16>, Vec<u16>> =
            image::ImageBuffer::from_raw(self.width, self.height, self.data.clone())
                .ok_or_else(|| anyhow::anyhow!("buffer size mismatch building Luma16"))?;
        buf.save(path)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Deterministic test fixture: diagonal ramp from 0 to u16::MAX. Cheap
    /// to regenerate, distinct from anything noise-like so render bugs
    /// (mirrored axes, swapped strides) jump out visually.
    pub fn synth_ramp(size: MapSize) -> Self {
        let (w, h) = size.heightmap_dims();
        let mut data = Vec::with_capacity((w as usize) * (h as usize));
        let denom = ((w - 1) + (h - 1)) as f32;
        for y in 0..h {
            for x in 0..w {
                let t = ((x + y) as f32) / denom;
                data.push((t * u16::MAX as f32) as u16);
            }
        }
        Self {
            width: w,
            height: h,
            data,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("heightmap dims {actual:?} do not match expected {expected:?} (must be 64·N+1)")]
pub struct DimMismatch {
    pub expected: (u32, u32),
    pub actual: (u32, u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_ramp_dims_match_map_size() {
        let m = MapSize::square(16);
        let h = Heightmap::synth_ramp(m);
        assert_eq!(h.dims(), (1025, 1025));
        h.validate_against(m).unwrap();
    }

    #[test]
    fn synth_ramp_spans_full_u16_range() {
        let h = Heightmap::synth_ramp(MapSize::square(2));
        let (lo, hi) = h.min_max();
        assert_eq!(lo, 0);
        // Top-right corner: t = ((w-1)+(h-1))/denom = 1.0 → u16::MAX.
        assert_eq!(hi, u16::MAX);
    }

    #[test]
    fn rejects_wrong_dims_against_map_size() {
        // 1024 is the classic mistake — power of two instead of 64·N+1.
        let bad = Heightmap::new(1024, 1024, vec![0; 1024 * 1024]).unwrap();
        let err = bad.validate_against(MapSize::square(16)).unwrap_err();
        assert_eq!(err.expected, (1025, 1025));
        assert_eq!(err.actual, (1024, 1024));
    }

    #[test]
    fn rejects_pixel_count_mismatch() {
        let err = Heightmap::new(10, 10, vec![0; 99]).unwrap_err();
        assert!(err.to_string().contains("pixel count mismatch"));
    }

    #[test]
    fn copy_rect_extracts_subregion() {
        let h = Heightmap::synth_ramp(MapSize::square(2));
        let sub = h.copy_rect(10, 20, 4, 3);
        assert_eq!(sub.len(), 12);
        // synth_ramp uses (x+y)/denom as the source — pixel (10, 20)
        // should match the raw read.
        let raw = h.data()[(20 * h.width() + 10) as usize];
        assert_eq!(sub[0], raw);
    }

    #[test]
    fn swap_rect_round_trips_contents() {
        let mut h = Heightmap::synth_ramp(MapSize::square(2));
        let orig = h.copy_rect(5, 7, 3, 2);
        let mut buf = vec![0u16; 6];
        h.swap_rect(5, 7, 3, 2, &mut buf);
        // After swap: heightmap rect is zero, buf holds the original.
        assert_eq!(buf, orig);
        let now = h.copy_rect(5, 7, 3, 2);
        assert!(now.iter().all(|&v| v == 0));
        // Swap back.
        h.swap_rect(5, 7, 3, 2, &mut buf);
        assert_eq!(h.copy_rect(5, 7, 3, 2), orig);
    }

    #[test]
    fn png_round_trip_preserves_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ramp.png");
        let h = Heightmap::synth_ramp(MapSize::square(2));
        h.save_png(&path).unwrap();
        let loaded = Heightmap::load_png(&path).unwrap();
        assert_eq!(h.dims(), loaded.dims());
        assert_eq!(h.data(), loaded.data());
    }
}
