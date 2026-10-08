//! The few choices that are the user's rather than a project's.

use std::path::Path;

use serde::{Deserialize, Serialize};

const FILE: &str = "settings.json";
const DEFAULT_SHORTCUT: &str = "alt+p";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Model given to new Claude Code sessions; empty leaves Claude Code's
    /// own default.
    pub claude_model: String,
    /// Same for Codex.
    pub codex_model: String,
    /// Where new projects are created; empty means Documents\Essaim.
    pub projects_dir: String,
    /// Studio's "Publish to Roblox" shortcut, for users who rebound it.
    pub publish_shortcut: String,
}

impl Settings {
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read(data_dir.join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        std::fs::write(data_dir.join(FILE), serde_json::to_vec_pretty(self)?)
    }

    /// The publish shortcut as the keys to hold together.
    pub fn publish_keys(&self) -> Vec<String> {
        let shortcut = if self.publish_shortcut.trim().is_empty() {
            DEFAULT_SHORTCUT
        } else {
            self.publish_shortcut.trim()
        };
        shortcut
            .split('+')
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_publish_shortcut_defaults_to_studios_own() {
        assert_eq!(Settings::default().publish_keys(), ["alt", "p"]);
        let custom = Settings { publish_shortcut: " Ctrl + Shift+P ".into(), ..Settings::default() };
        assert_eq!(custom.publish_keys(), ["Ctrl", "Shift", "P"]);
    }

    #[test]
    fn settings_survive_a_restart_and_old_files_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings { claude_model: "sonnet".into(), ..Settings::default() };
        settings.save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()).claude_model, "sonnet");

        // A file written before a setting existed simply lacks it.
        std::fs::write(dir.path().join(FILE), r#"{"codex_model":"x"}"#).unwrap();
        let loaded = Settings::load(dir.path());
        assert_eq!((loaded.codex_model.as_str(), loaded.claude_model.as_str()), ("x", ""));
    }
}
