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
    /// Where new projects are created; empty means Documents\RoVibe.
    pub projects_dir: String,
    /// Studio's "Publish to Roblox" shortcut, for users who rebound it.
    pub publish_shortcut: String,
    /// `open` lets isolated agents reach the whole internet; anything else
    /// keeps them to the model's API and the hosts below.
    pub isolation_network: String,
    /// Hosts isolated agents may reach besides the model's API, e.g.
    /// `github.com *.githubusercontent.com`.
    pub isolation_hosts: String,
    /// Windows notifications: empty for all of them, `waiting` for agents
    /// that need the user only, `off` for none.
    pub notifications: String,
}

/// What Claude Code needs to log in and to work: Anthropic's API and sign-in
/// pages, and nothing a project's code or assets could be sent to.
const MODEL_HOSTS: [&str; 6] = [
    "anthropic.com",
    "*.anthropic.com",
    "claude.ai",
    "*.claude.ai",
    "claude.com",
    "*.claude.com",
];

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

    pub fn isolation_restricted(&self) -> bool {
        self.isolation_network.trim() != "open"
    }

    /// Every host an isolated agent may reach. Whatever the user typed is
    /// reduced to host names: the list ends up in a file read by a proxy and
    /// in a shell script run as root.
    pub fn isolation_allowed_hosts(&self) -> Vec<String> {
        let extra = self
            .isolation_hosts
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .map(|host| {
                let host = host.trim().to_lowercase();
                let host = host
                    .strip_prefix("https://")
                    .or_else(|| host.strip_prefix("http://"))
                    .unwrap_or(&host);
                host.split('/').next().unwrap_or_default().to_owned()
            })
            .filter(|host| is_host_pattern(host));
        let mut hosts: Vec<String> = MODEL_HOSTS.iter().map(|host| (*host).to_owned()).collect();
        for host in extra {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        hosts
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

/// A host name, or `*.` followed by one. A bare `*` or a top-level domain
/// would open everything, which is what the `open` setting is for.
fn is_host_pattern(host: &str) -> bool {
    let name = host.strip_prefix("*.").unwrap_or(host);
    name.contains('.')
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
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
    fn isolated_agents_reach_the_model_and_what_the_user_adds() {
        let default = Settings::default();
        assert!(default.isolation_restricted());
        assert!(default.isolation_allowed_hosts().contains(&"*.anthropic.com".to_owned()));

        let custom = Settings {
            isolation_hosts: "GitHub.com, https://api.github.com/repos *.githubusercontent.com\n* *.com evil;rm -rf / $(id) a..b -x.fr claude.ai".into(),
            ..Settings::default()
        };
        let added: Vec<String> = custom.isolation_allowed_hosts().split_off(MODEL_HOSTS.len());
        assert_eq!(added, ["github.com", "api.github.com", "*.githubusercontent.com"]);

        let open = Settings { isolation_network: "open".into(), ..Settings::default() };
        assert!(!open.isolation_restricted());
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
