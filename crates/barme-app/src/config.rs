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

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Current editor version. The first-launch hint replays once per
/// distinct value seen.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Hard cap on the recent-projects list. The submenu only renders the
/// first 10 entries anyway; nothing keeps a longer list around.
pub const RECENT_PROJECTS_CAP: usize = 10;

/// Schema for `barme-editor.toml`. Forward-compat: every field has
/// `#[serde(default)]` so older configs load cleanly, and unknown TOML
/// keys are ignored (default `toml` behaviour).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorConfig {
    /// Editor versions that have already shown the first-launch hint.
    /// Appending a new value replays the hint once.
    #[serde(default)]
    pub seen_intro_versions: Vec<String>,

    /// Sprint 20: most-recently-opened or saved project files. Capped at
    /// [`RECENT_PROJECTS_CAP`]. Front = most recent. Persists across
    /// editor restarts; missing files drop silently at next open
    /// (`App::open_from` → [`EditorConfig::remove_recent`]).
    #[serde(default)]
    pub recent_projects: VecDeque<PathBuf>,

    /// Sprint 22 / U2: editor version for which the guided tour has
    /// been completed (or skipped). `None` = tour hasn't run for
    /// this version → the first new project auto-triggers it; once
    /// completed/skipped, the value is set to [`CURRENT_VERSION`].
    /// A new editor release re-arms the tour by changing
    /// [`CURRENT_VERSION`].
    #[serde(default)]
    pub tour_completed_for: Option<String>,

    /// Sprint 22 / U2: set of tool keyboard accelerators (e.g.
    /// `"B"`, `"L"`) whose per-tool intro overlay the user has
    /// explicitly dismissed via the "Don't show again" checkbox.
    /// Esc-dismiss does NOT add to this set — Esc means "I get it
    /// for now; show me next time". `BTreeSet` for
    /// deterministic TOML serialisation order.
    #[serde(default)]
    pub tool_intros_seen: BTreeSet<String>,
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

    /// Sprint 20: record an opened-or-saved `.barmeproj`. The path is
    /// promoted to the front of the list; any duplicate further back
    /// is removed; the list is truncated to [`RECENT_PROJECTS_CAP`].
    /// Idempotent if `path` is already at the front.
    pub fn push_recent(&mut self, path: PathBuf) {
        // Dedupe by exact path; preserves the user's mental model of
        // "this is the same project I just had open."
        self.recent_projects.retain(|p| p != &path);
        self.recent_projects.push_front(path);
        while self.recent_projects.len() > RECENT_PROJECTS_CAP {
            self.recent_projects.pop_back();
        }
    }

    /// Sprint 20: drop `path` from the list (e.g. open failed because
    /// the file no longer exists). Silent — Sprint 31's toast queue
    /// can promote the dropped-entry notification later.
    pub fn remove_recent(&mut self, path: &Path) {
        self.recent_projects.retain(|p| p != path);
    }

    /// Sprint 20: drop every entry. Wired to the File > Recent
    /// projects > Clear menu item.
    pub fn clear_recent(&mut self) {
        self.recent_projects.clear();
    }

    /// Sprint 22 / U2: has the guided tour been completed (or
    /// skipped) for `version`? Identity equality on the stored
    /// string.
    #[allow(dead_code)] // wired by App::update + tour::start in commit 2
    pub fn tour_completed_for(&self, version: &str) -> bool {
        self.tour_completed_for
            .as_deref()
            .map(|v| v == version)
            .unwrap_or(false)
    }

    /// Convenience: has the tour been completed for the
    /// *currently running* editor version?
    #[allow(dead_code)] // wired by App::update in commit 2
    pub fn tour_completed_for_current_version(&self) -> bool {
        self.tour_completed_for(CURRENT_VERSION)
    }

    /// Sprint 22 / U2: record that the user finished the guided
    /// tour (or hit "Skip tour") for `version`. Replays once on
    /// the next version bump.
    #[allow(dead_code)] // wired by tour completion in commit 2
    pub fn mark_tour_completed_for(&mut self, version: &str) {
        self.tour_completed_for = Some(version.to_string());
    }

    /// Convenience: mark current-editor-version tour as completed.
    #[allow(dead_code)] // wired by tour completion in commit 2
    pub fn mark_tour_completed_for_current_version(&mut self) {
        self.mark_tour_completed_for(CURRENT_VERSION);
    }

    /// Sprint 22 / U2: re-arm the guided tour. Help menu's
    /// "Start guided tour" item flips this so the next App
    /// frame replays it from step 1.
    #[allow(dead_code)] // wired by Help menu in commit 6
    pub fn reset_tour_completion(&mut self) {
        self.tour_completed_for = None;
    }

    /// Sprint 22 / U2: has the per-tool intro for `accel` been
    /// dismissed via the explicit "Don't show again" checkbox?
    /// Esc-dismiss returns `false` (intentional — Esc is "for
    /// now", not "forever").
    #[allow(dead_code)] // wired by tool-intro overlay in commit 3
    pub fn tool_intro_seen(&self, accel: &str) -> bool {
        self.tool_intros_seen.contains(accel)
    }

    /// Sprint 22 / U2: record that the user pinned `accel`'s
    /// intro to "Don't show again".
    #[allow(dead_code)] // wired by tool-intro overlay in commit 3
    pub fn mark_tool_intro_seen(&mut self, accel: &str) {
        self.tool_intros_seen.insert(accel.to_string());
    }

    /// Sprint 22 / U2: re-arm every per-tool intro. Help menu's
    /// "Reset tool intros" item calls this.
    #[allow(dead_code)] // wired by Help menu in commit 6
    pub fn reset_tool_intros(&mut self) {
        self.tool_intros_seen.clear();
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

    // ─── Sprint 20 / chunk 7 ─────────────────────────────────────────

    #[test]
    fn push_recent_promotes_to_front_and_dedupes() {
        let mut cfg = EditorConfig::default();
        cfg.push_recent(PathBuf::from("/a.barmeproj"));
        cfg.push_recent(PathBuf::from("/b.barmeproj"));
        cfg.push_recent(PathBuf::from("/c.barmeproj"));
        // b is promoted, ahead of c, behind itself; dedupe drops the
        // older b position.
        cfg.push_recent(PathBuf::from("/b.barmeproj"));
        assert_eq!(
            cfg.recent_projects.iter().collect::<Vec<_>>(),
            vec![
                &PathBuf::from("/b.barmeproj"),
                &PathBuf::from("/c.barmeproj"),
                &PathBuf::from("/a.barmeproj"),
            ]
        );
    }

    #[test]
    fn push_recent_truncates_at_cap() {
        let mut cfg = EditorConfig::default();
        for i in 0..RECENT_PROJECTS_CAP + 5 {
            cfg.push_recent(PathBuf::from(format!("/{i}.barmeproj")));
        }
        assert_eq!(cfg.recent_projects.len(), RECENT_PROJECTS_CAP);
        // The most-recent (i=14) sits at the front.
        let front = cfg.recent_projects.front().unwrap();
        assert_eq!(
            front,
            &PathBuf::from(format!("/{}.barmeproj", RECENT_PROJECTS_CAP + 4))
        );
        // The oldest survivor is i=5 (5..14 = 10 entries).
        let back = cfg.recent_projects.back().unwrap();
        assert_eq!(back, &PathBuf::from("/5.barmeproj"));
    }

    #[test]
    fn remove_recent_drops_only_matching_path() {
        let mut cfg = EditorConfig::default();
        cfg.push_recent(PathBuf::from("/a.barmeproj"));
        cfg.push_recent(PathBuf::from("/b.barmeproj"));
        cfg.push_recent(PathBuf::from("/c.barmeproj"));
        cfg.remove_recent(Path::new("/b.barmeproj"));
        assert_eq!(cfg.recent_projects.len(), 2);
        assert!(cfg.recent_projects.contains(&PathBuf::from("/a.barmeproj")));
        assert!(cfg.recent_projects.contains(&PathBuf::from("/c.barmeproj")));
        assert!(!cfg.recent_projects.contains(&PathBuf::from("/b.barmeproj")));
    }

    #[test]
    fn clear_recent_empties_the_list() {
        let mut cfg = EditorConfig::default();
        cfg.push_recent(PathBuf::from("/a.barmeproj"));
        cfg.push_recent(PathBuf::from("/b.barmeproj"));
        cfg.clear_recent();
        assert!(cfg.recent_projects.is_empty());
    }

    #[test]
    fn recent_projects_round_trip_through_disk() {
        let mut cfg = EditorConfig::default();
        cfg.push_recent(PathBuf::from("/a.barmeproj"));
        cfg.push_recent(PathBuf::from("/b.barmeproj"));
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("recent.toml");
        cfg.save_to(&path).expect("save");
        let loaded = EditorConfig::load_from(&path).expect("load");
        assert_eq!(loaded.recent_projects, cfg.recent_projects);
    }

    #[test]
    fn recent_projects_loads_from_older_config_via_serde_default() {
        // A config file written before Sprint 20 has only the
        // `seen_intro_versions` field; recent_projects must default
        // cleanly.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("legacy.toml");
        std::fs::write(&path, "seen_intro_versions = [\"0.0.1\"]\n").unwrap();
        let loaded = EditorConfig::load_from(&path).expect("load");
        assert!(loaded.recent_projects.is_empty());
        assert_eq!(loaded.seen_intro_versions, vec!["0.0.1"]);
    }

    // ─── Sprint 22 / U2 — tour + tool-intro persistence ──────────────

    #[test]
    fn tour_completion_round_trips_through_disk() {
        let mut cfg = EditorConfig::default();
        assert!(!cfg.tour_completed_for_current_version());
        cfg.mark_tour_completed_for("0.0.7");
        assert!(cfg.tour_completed_for("0.0.7"));
        assert!(!cfg.tour_completed_for("0.0.8"));

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tour.toml");
        cfg.save_to(&path).expect("save");
        let loaded = EditorConfig::load_from(&path).expect("load");
        assert!(loaded.tour_completed_for("0.0.7"));
    }

    #[test]
    fn reset_tour_re_arms_the_walkthrough() {
        let mut cfg = EditorConfig::default();
        cfg.mark_tour_completed_for_current_version();
        assert!(cfg.tour_completed_for_current_version());
        cfg.reset_tour_completion();
        assert!(!cfg.tour_completed_for_current_version());
    }

    #[test]
    fn tool_intro_seen_records_and_resets() {
        let mut cfg = EditorConfig::default();
        assert!(!cfg.tool_intro_seen("L"));
        cfg.mark_tool_intro_seen("L");
        cfg.mark_tool_intro_seen("B");
        assert!(cfg.tool_intro_seen("L"));
        assert!(cfg.tool_intro_seen("B"));
        assert!(!cfg.tool_intro_seen("Q"));
        cfg.reset_tool_intros();
        assert!(!cfg.tool_intro_seen("L"));
    }

    #[test]
    fn tool_intros_round_trip_through_disk() {
        let mut cfg = EditorConfig::default();
        cfg.mark_tool_intro_seen("L");
        cfg.mark_tool_intro_seen("B");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("intros.toml");
        cfg.save_to(&path).expect("save");
        let loaded = EditorConfig::load_from(&path).expect("load");
        assert!(loaded.tool_intro_seen("L"));
        assert!(loaded.tool_intro_seen("B"));
    }

    #[test]
    fn legacy_config_without_tour_fields_loads_clean() {
        // Sprint 21 and earlier wrote configs without
        // `tour_completed_for` / `tool_intros_seen`. They must
        // default cleanly under serde(default).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("preU2.toml");
        std::fs::write(
            &path,
            "seen_intro_versions = [\"0.0.6\"]\nrecent_projects = []\n",
        )
        .unwrap();
        let loaded = EditorConfig::load_from(&path).expect("load");
        assert!(loaded.tour_completed_for.is_none());
        assert!(loaded.tool_intros_seen.is_empty());
    }
}
