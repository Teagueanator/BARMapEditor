//! Project root — the editable, on-disk representation of a map under construction.
//!
//! Persisted as `<name>.barmeproj` (TOML manifest) plus a sibling directory of
//! raw asset PNGs (heightmap, metal, type, splat distribution, diffuse). The
//! `.sd7` is build output, not source of truth.

use serde::{Deserialize, Serialize};

use crate::MapSize;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub size: MapSize,
    pub min_height: f32,
    pub max_height: f32,
}

impl Project {
    pub fn new(name: impl Into<String>, smu: u32) -> Self {
        Self {
            name: name.into(),
            size: MapSize::square(smu),
            min_height: 0.0,
            max_height: 256.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trips_through_toml() {
        let p = Project::new("apophis-clone", 16);
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.name, p2.name);
        assert_eq!(p.size, p2.size);
    }
}
