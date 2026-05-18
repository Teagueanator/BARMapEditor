//! Project root — the editable, on-disk representation of a map under construction.
//!
//! Persisted as `<name>.barmeproj` (TOML manifest) plus a sibling directory of
//! raw asset PNGs (heightmap, metal, type, splat distribution, diffuse). The
//! `.sd7` is build output, not source of truth.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::MapSize;

/// File extension for the project manifest (no leading dot).
pub const PROJECT_EXTENSION: &str = "barmeproj";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub size: MapSize,
    pub min_height: f32,
    pub max_height: f32,
    /// Path to the heightmap PNG. Relative paths resolve against the project
    /// file's parent directory (see [`Project::resolve_heightmap`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heightmap: Option<PathBuf>,
    /// Team start positions (F8 / ADR-023). Empty in legacy projects;
    /// `#[serde(default)]` lets them load forward. The pipeline emits these
    /// into `mapinfo.lua` `teams[]` when non-empty, or falls back to a
    /// 25%/75% diagonal pair when empty so blank projects still build a
    /// playable 1v1 map.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub start_positions: Vec<StartPosition>,
    /// User-authored `mapinfo.lua` field overrides (C1 / ADR-028).
    /// Populated by the F9 form editor (C7) on top of the
    /// `MapInfo::bar_default()` baseline so unusual maps (skybox
    /// changes, custom gadget `custom.*` blobs, etc.) can ship without
    /// schema bumps. Keys are dotted Lua paths
    /// (e.g. `"atmosphere.sky_box"`); values are TOML scalars / arrays
    /// the emitter knows how to render. Default empty.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mapinfo_overrides: HashMap<String, toml::Value>,
}

/// A single team start position in world coordinates (elmos).
///
/// `team_id` indexes the `teams[]` table in `mapinfo.lua` — BAR's per-side
/// convention is even IDs on side A, odd IDs on side B, so the F8 editor
/// auto-assigns mirrors `{0,1}`, `{2,3}`, etc. when symmetry is enabled.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartPosition {
    pub team_id: u8,
    pub x_elmo: u32,
    pub z_elmo: u32,
}

#[derive(Debug, Error)]
pub enum ProjectLoadError {
    #[error("read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[derive(Debug, Error)]
pub enum ProjectSaveError {
    #[error("serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Sanitize a user-entered project name into a form safe for the
/// downstream pipeline (filenames, `mapinfo.lua` strings, `.sd7` archive
/// names). Allowed characters: ASCII alphanumeric, `_`, `-`. Anything
/// else is collapsed into a single `_`. Leading/trailing `_` are trimmed.
/// Empty input maps to `"untitled"`.
///
/// The pipeline already escapes `"` and `\` in Lua emit (`lua_string`),
/// but a name like `"my map: 1.0"` would otherwise produce an `.sd7` with
/// a colon in its filename and trigger PITFALL #7 (pink-map on rename)
/// in subtle ways. Sanitizing at the project boundary is defence-in-depth.
pub fn sanitize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
            last_was_underscore = c == '_';
        } else if !last_was_underscore && !out.is_empty() {
            out.push('_');
            last_was_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

impl Project {
    pub fn new(name: impl Into<String>, smu: u32) -> Self {
        Self {
            name: name.into(),
            size: MapSize::square(smu),
            min_height: 0.0,
            max_height: 256.0,
            heightmap: None,
            start_positions: Vec::new(),
            mapinfo_overrides: HashMap::new(),
        }
    }

    /// Resolve `heightmap` against the project file's parent directory.
    /// Returns `None` if no heightmap is set.
    pub fn resolve_heightmap(&self, project_path: &Path) -> Option<PathBuf> {
        let rel = self.heightmap.as_ref()?;
        if rel.is_absolute() {
            return Some(rel.clone());
        }
        let base = project_path.parent().unwrap_or_else(|| Path::new("."));
        Some(base.join(rel))
    }

    /// Rewrite `heightmap` as relative to the project file's parent when
    /// possible. Falls back to the absolute path otherwise. Call before save.
    pub fn relativize_heightmap(&mut self, project_path: &Path) {
        let Some(abs) = self.heightmap.as_ref() else {
            return;
        };
        if !abs.is_absolute() {
            return;
        }
        let Some(base) = project_path.parent() else {
            return;
        };
        if let Ok(rel) = abs.strip_prefix(base) {
            self.heightmap = Some(rel.to_path_buf());
        }
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), ProjectSaveError> {
        let s = toml::to_string_pretty(self)?;
        fs::write(path, s).map_err(|e| ProjectSaveError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    }

    pub fn load_from_file(path: &Path) -> Result<Self, ProjectLoadError> {
        let s = fs::read_to_string(path).map_err(|e| ProjectLoadError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        toml::from_str(&s).map_err(|e| ProjectLoadError::Parse {
            path: path.to_path_buf(),
            source: e,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trips_through_toml() {
        let mut p = Project::new("apophis-clone", 16);
        p.heightmap = Some(PathBuf::from("heightmap.png"));
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.name, p2.name);
        assert_eq!(p.size, p2.size);
        assert_eq!(p.min_height, p2.min_height);
        assert_eq!(p.max_height, p2.max_height);
        assert_eq!(p.heightmap, p2.heightmap);
    }

    #[test]
    fn heightmap_omitted_when_none() {
        let p = Project::new("no-hm", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("heightmap"), "got:\n{s}");
    }

    #[test]
    fn start_positions_omitted_when_empty() {
        let p = Project::new("no-teams", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(!s.contains("start_positions"), "got:\n{s}");
    }

    #[test]
    fn start_positions_round_trip() {
        let mut p = Project::new("teams", 8);
        p.start_positions = vec![
            StartPosition {
                team_id: 0,
                x_elmo: 1024,
                z_elmo: 1024,
            },
            StartPosition {
                team_id: 1,
                x_elmo: 3072,
                z_elmo: 3072,
            },
        ];
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.start_positions, p2.start_positions);
    }

    #[test]
    fn mapinfo_overrides_omitted_when_empty() {
        let p = Project::new("clean", 4);
        let s = toml::to_string(&p).unwrap();
        assert!(
            !s.contains("mapinfo_overrides"),
            "empty mapinfo_overrides must not serialise; got:\n{s}"
        );
    }

    #[test]
    fn mapinfo_overrides_round_trip() {
        let mut p = Project::new("overrides", 4);
        p.mapinfo_overrides
            .insert("atmosphere.sky_box".to_string(), "clear_day.dds".into());
        p.mapinfo_overrides
            .insert("gravity".to_string(), toml::Value::Float(150.0));
        let s = toml::to_string(&p).unwrap();
        let p2: Project = toml::from_str(&s).unwrap();
        assert_eq!(p.mapinfo_overrides, p2.mapinfo_overrides);
    }

    #[test]
    fn legacy_project_without_mapinfo_overrides_loads_forward() {
        // Pre-C1 projects don't carry the field. serde(default) seeds
        // an empty map.
        let toml_str = r#"
name = "legacy"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert!(p.mapinfo_overrides.is_empty());
    }

    #[test]
    fn legacy_project_without_start_positions_loads_forward() {
        let toml_str = r#"
name = "legacy"
min_height = 0.0
max_height = 256.0

[size]
smu_x = 4
smu_z = 4
"#;
        let p: Project = toml::from_str(toml_str).unwrap();
        assert_eq!(p.name, "legacy");
        assert!(p.start_positions.is_empty());
    }

    #[test]
    fn save_and_load_round_trip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.barmeproj");
        let p = Project::new("demo", 8);
        p.save_to_file(&path).unwrap();
        let p2 = Project::load_from_file(&path).unwrap();
        assert_eq!(p.name, p2.name);
        assert_eq!(p.size, p2.size);
    }

    #[test]
    fn sanitize_name_passes_alphanumeric_through() {
        assert_eq!(sanitize_name("apophis-clone-1"), "apophis-clone-1");
        assert_eq!(sanitize_name("my_map_v2"), "my_map_v2");
    }

    #[test]
    fn sanitize_name_collapses_disallowed_into_underscore() {
        assert_eq!(sanitize_name("my map: 1.0"), "my_map_1_0");
        assert_eq!(sanitize_name("hello!!!world"), "hello_world");
    }

    #[test]
    fn sanitize_name_trims_edges_and_handles_empty() {
        assert_eq!(sanitize_name("   "), "untitled");
        assert_eq!(sanitize_name(""), "untitled");
        assert_eq!(sanitize_name("___"), "untitled");
        assert_eq!(sanitize_name(" foo "), "foo");
    }

    #[test]
    fn sanitize_name_creates_safe_filenames() {
        // Anything sanitize produces must be a legal portable filename
        // (no path separators, no colons, no spaces).
        for input in ["maps/foo", "C:\\Users\\me", "a b c", "x/y\\z"] {
            let s = sanitize_name(input);
            assert!(!s.contains('/'));
            assert!(!s.contains('\\'));
            assert!(!s.contains(':'));
            assert!(!s.contains(' '));
        }
    }

    #[test]
    fn relativize_heightmap_strips_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("demo.barmeproj");
        let hm_abs = dir.path().join("heightmap.png");
        let mut p = Project::new("demo", 4);
        p.heightmap = Some(hm_abs.clone());
        p.relativize_heightmap(&project_path);
        assert_eq!(p.heightmap, Some(PathBuf::from("heightmap.png")));
        let resolved = p.resolve_heightmap(&project_path).unwrap();
        assert_eq!(resolved, hm_abs);
    }
}
