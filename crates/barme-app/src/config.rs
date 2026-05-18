//! Per-user, per-OS editor configuration TOML. Distinct from
//! `.barmeproj` (per-project) — this is the "this user has seen the
//! intro" / future theme / future window-state surface.
//!
//! Path: `directories::ProjectDirs::config_dir() / "barme-editor.toml"`.
//!
//! Phase-3 pitfall §B3.4: the first-launch hint flag belongs here, not
//! in the project file. Versioned with the editor — a major release
//! replays the intro once by appending a new version string to
//! [`EditorConfig::seen_intro_versions`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Current editor version. The first-launch hint replays once per
/// distinct value seen.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Schema for `barme-editor.toml`. Forward-compat: every field has
/// `#[serde(default)]` so older configs load cleanly, and unknown TOML
/// keys are ignored (default `toml` behaviour).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorConfig {
    /// Editor versions that have already shown the first-launch hint.
    /// Appending a new value replays the hint once.
    #[serde(default)]
    pub seen_intro_versions: Vec<String>,
}

impl EditorConfig {
    /// Default per-user config path, or `None` if the OS doesn't
    /// expose a config dir (rare; sandboxed environments).
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "barme", "barme-editor")
            .map(|p| p.config_dir().join("barme-editor.toml"))
    }

    /// Load from the OS-standard path. Returns `Default` (no intro
    /// versions) on missing file, parse error, or no config dir. All
    /// errors are non-fatal — the editor still runs, the user just
    /// sees the first-launch hint again.
    pub fn load() -> Self {
        let Some(path) = Self::default_path() else {
            warn!("no per-user config dir available; first-launch hint will replay each launch");
            return Self::default();
        };
        match Self::load_from(&path) {
            Ok(cfg) => {
                info!(path = %path.display(), "editor config loaded");
                cfg
            }
            Err(e) => {
                // Missing file is the cold-start case — info, not warn.
                if path.exists() {
                    warn!(path = %path.display(), error = %format!("{e:#}"), "editor config load failed; using defaults");
                } else {
                    info!(path = %path.display(), "no editor config yet; using defaults");
                }
                Self::default()
            }
        }
    }

    /// Save to the OS-standard path. Creates parent dirs as needed.
    /// Logs errors at `warn` rather than propagating — saving is best-
    /// effort UX state, not load-bearing data.
    pub fn save(&self) {
        let Some(path) = Self::default_path() else {
            warn!("no per-user config dir available; editor config not saved");
            return;
        };
        if let Err(e) = self.save_to(&path) {
            warn!(path = %path.display(), error = %format!("{e:#}"), "editor config save failed");
        }
    }

    /// Load from an explicit path. Public for tests.
    pub fn load_from(path: &Path) -> Result<Self> {
        let body =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let cfg: Self = toml::from_str(&body)
            .with_context(|| format!("parse {} as EditorConfig TOML", path.display()))?;
        Ok(cfg)
    }

    /// Save to an explicit path. Public for tests.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create_dir_all {}", parent.display()))?;
        }
        let body = toml::to_string_pretty(self).context("serialise EditorConfig to TOML")?;
        std::fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Has the first-launch hint been dismissed for `version`?
    pub fn intro_seen_for(&self, version: &str) -> bool {
        self.seen_intro_versions.iter().any(|v| v == version)
    }

    /// Record that the user dismissed the hint for `version`. Idempotent.
    pub fn mark_intro_seen_for(&mut self, version: &str) {
        if !self.intro_seen_for(version) {
            self.seen_intro_versions.push(version.to_string());
        }
    }

    /// Convenience: was the hint dismissed for the *currently running*
    /// editor version?
    pub fn intro_seen_for_current_version(&self) -> bool {
        self.intro_seen_for(CURRENT_VERSION)
    }

    /// Convenience: mark the *currently running* editor version's hint
    /// as seen.
    pub fn mark_intro_seen_for_current_version(&mut self) {
        self.mark_intro_seen_for(CURRENT_VERSION);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_seen_intros() {
        let cfg = EditorConfig::default();
        assert!(cfg.seen_intro_versions.is_empty());
        assert!(!cfg.intro_seen_for("0.0.1"));
        assert!(!cfg.intro_seen_for_current_version());
    }

    #[test]
    fn mark_then_check_returns_true_for_marked_version() {
        let mut cfg = EditorConfig::default();
        cfg.mark_intro_seen_for("0.0.1");
        assert!(cfg.intro_seen_for("0.0.1"));
        assert!(!cfg.intro_seen_for("0.0.2"));
    }

    #[test]
    fn mark_is_idempotent_per_version() {
        let mut cfg = EditorConfig::default();
        cfg.mark_intro_seen_for("0.0.1");
        cfg.mark_intro_seen_for("0.0.1");
        cfg.mark_intro_seen_for("0.0.1");
        assert_eq!(cfg.seen_intro_versions.len(), 1);
    }

    #[test]
    fn mark_appends_distinct_versions() {
        let mut cfg = EditorConfig::default();
        cfg.mark_intro_seen_for("0.0.1");
        cfg.mark_intro_seen_for("0.0.2");
        assert_eq!(cfg.seen_intro_versions, vec!["0.0.1", "0.0.2"]);
    }

    #[test]
    fn mark_current_version_uses_cargo_pkg_version() {
        let mut cfg = EditorConfig::default();
        cfg.mark_intro_seen_for_current_version();
        assert!(cfg.intro_seen_for_current_version());
        assert!(cfg.intro_seen_for(CURRENT_VERSION));
    }

    #[test]
    fn round_trip_through_disk() {
        let mut cfg = EditorConfig::default();
        cfg.mark_intro_seen_for("0.0.1");
        cfg.mark_intro_seen_for("0.0.2");

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sub").join("barme-editor.toml");
        cfg.save_to(&path).expect("save");
        assert!(path.exists());

        let loaded = EditorConfig::load_from(&path).expect("load");
        assert_eq!(loaded, cfg);
        assert!(loaded.intro_seen_for("0.0.1"));
        assert!(loaded.intro_seen_for("0.0.2"));
    }

    #[test]
    fn load_from_missing_file_returns_err() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nonexistent.toml");
        let result = EditorConfig::load_from(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_from_malformed_toml_returns_err() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is = = not [[ valid toml").expect("write");
        let result = EditorConfig::load_from(&path);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_keys_are_ignored_forward_compat() {
        // A future version may add a field we don't know yet. Older
        // editor loads should still parse cleanly.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("future.toml");
        std::fs::write(
            &path,
            "seen_intro_versions = [\"0.0.1\"]\n\
             future_field = \"some value\"\n\
             future_table = { a = 1, b = 2 }\n",
        )
        .expect("write");
        let loaded = EditorConfig::load_from(&path).expect("load forward-compat");
        assert_eq!(loaded.seen_intro_versions, vec!["0.0.1"]);
    }

    #[test]
    fn save_then_reload_preserves_order() {
        let mut cfg = EditorConfig::default();
        // The order is the order versions were dismissed in; tests pin
        // it so a future serialiser change that scrambled order would
        // be visible.
        cfg.mark_intro_seen_for("0.0.5");
        cfg.mark_intro_seen_for("0.0.1");
        cfg.mark_intro_seen_for("0.0.3");

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ord.toml");
        cfg.save_to(&path).expect("save");
        let loaded = EditorConfig::load_from(&path).expect("load");
        assert_eq!(loaded.seen_intro_versions, vec!["0.0.5", "0.0.1", "0.0.3"]);
    }

    #[test]
    fn current_version_const_matches_cargo_pkg_version() {
        // Belt-and-braces — a future env macro change would flip this.
        assert_eq!(CURRENT_VERSION, env!("CARGO_PKG_VERSION"));
    }
}
