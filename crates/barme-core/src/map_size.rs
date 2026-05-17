//! Spring Map Unit (SMU) dimensions.
//!
//! 1 SMU = 512 px texture = 64 px heightmap edge = 512 elmos (world units).
//! Maps are square in SMU; a "16×16 map" has `smu = (16, 16)`.
//! Heightmap edge is `64·N + 1` — the off-by-one is the #1 silent failure mode (SRS §2.1).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapSize {
    pub smu_x: u32,
    pub smu_z: u32,
}

impl MapSize {
    pub const ELMOS_PER_SMU: u32 = 512;
    pub const HEIGHTMAP_PER_SMU: u32 = 64;
    pub const TEXTURE_PER_SMU: u32 = 512;
    pub const METAL_PER_SMU: u32 = 32;
    pub const GRASS_PER_SMU: u32 = 16;

    pub const fn square(smu: u32) -> Self {
        Self {
            smu_x: smu,
            smu_z: smu,
        }
    }

    /// Heightmap edge length in pixels: `64·N + 1`.
    pub const fn heightmap_dims(&self) -> (u32, u32) {
        (
            self.smu_x * Self::HEIGHTMAP_PER_SMU + 1,
            self.smu_z * Self::HEIGHTMAP_PER_SMU + 1,
        )
    }

    /// Diffuse texture dimensions in pixels: `512·N`.
    pub const fn texture_dims(&self) -> (u32, u32) {
        (
            self.smu_x * Self::TEXTURE_PER_SMU,
            self.smu_z * Self::TEXTURE_PER_SMU,
        )
    }

    /// Metal / type map dimensions: `32·N`.
    pub const fn metal_dims(&self) -> (u32, u32) {
        (
            self.smu_x * Self::METAL_PER_SMU,
            self.smu_z * Self::METAL_PER_SMU,
        )
    }

    pub const fn grass_dims(&self) -> (u32, u32) {
        (
            self.smu_x * Self::GRASS_PER_SMU,
            self.smu_z * Self::GRASS_PER_SMU,
        )
    }

    pub const fn elmo_extents(&self) -> (u32, u32) {
        (
            self.smu_x * Self::ELMOS_PER_SMU,
            self.smu_z * Self::ELMOS_PER_SMU,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_by_sixteen_matches_srs() {
        let s = MapSize::square(16);
        assert_eq!(s.heightmap_dims(), (1025, 1025));
        assert_eq!(s.texture_dims(), (8192, 8192));
        assert_eq!(s.metal_dims(), (512, 512));
        assert_eq!(s.grass_dims(), (256, 256));
        assert_eq!(s.elmo_extents(), (8192, 8192));
    }
}
