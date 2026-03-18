//! JSON allowlist authentication for channels.
//! File: ~/.clawbro/allowlist.json (or CLAWBRO_ALLOWLIST_PATH env var).
//! If file absent → open mode (everyone allowed).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Per-channel allowlist config
#[derive(Debug, Deserialize, Default)]
struct ChannelConfig {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    mode: String, // "open" | "allowlist"; default = "open"
    #[serde(default)]
    users: Vec<String>, // DingTalk user_ids
    #[serde(default)]
    open_ids: Vec<String>, // Lark open_ids
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Default)]
struct AllowlistFile {
    #[serde(default, flatten)]
    channels: HashMap<String, ChannelConfig>,
}

/// Allowlist checker — load once at startup, check per message.
pub struct AllowlistChecker {
    channels: HashMap<String, ChannelConfig>,
}

impl AllowlistChecker {
    /// Load from env var path → default path → open mode if absent.
    pub fn load() -> Self {
        let path = std::env::var("CLAWBRO_ALLOWLIST_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(".clawbro")
                    .join("allowlist.json")
            });
        Self::from_path(Some(&path))
    }

    /// Load from a specific path (used in tests).
    pub fn from_path<P: AsRef<Path>>(path: Option<P>) -> Self {
        let channels = path
            .and_then(|p| {
                let s = std::fs::read_to_string(p.as_ref()).ok()?;
                match serde_json::from_str::<AllowlistFile>(&s) {
                    Ok(f) => Some(f.channels),
                    Err(e) => {
                        tracing::warn!(
                            "allowlist.json parse error, falling back to open mode: {e}"
                        );
                        None
                    }
                }
            })
            .unwrap_or_default();
        Self { channels }
    }

    /// Returns true if the user is allowed to use this channel.
    pub fn is_allowed(&self, channel: &str, user_id: &str) -> bool {
        match self.channels.get(channel) {
            None => true, // channel not configured → open mode
            Some(cfg) => {
                if !cfg.enabled {
                    return false;
                }
                if cfg.mode == "allowlist" {
                    cfg.users.iter().any(|u| u == user_id)
                        || cfg.open_ids.iter().any(|u| u == user_id)
                } else {
                    if !cfg.mode.is_empty() && cfg.mode != "open" {
                        tracing::warn!(
                            "allowlist: unrecognized mode {:?} for channel {}, treating as open",
                            cfg.mode,
                            channel
                        );
                    }
                    true
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn test_open_mode_no_file() {
        let checker = AllowlistChecker::from_path(None::<std::path::PathBuf>);
        assert!(checker.is_allowed("dingtalk", "any_user"));
        assert!(checker.is_allowed("lark", "ou_xxx"));
    }

    #[test]
    fn test_open_mode_explicit() {
        let json = r#"{"dingtalk": {"enabled": true, "mode": "open"}}"#;
        let f = write_tmp(json);
        let checker = AllowlistChecker::from_path(Some(f.path()));
        assert!(checker.is_allowed("dingtalk", "any_user"));
    }

    #[test]
    fn test_allowlist_mode_allowed() {
        let json = r#"{
            "lark": {"enabled": true, "mode": "allowlist", "open_ids": ["ou_abc", "ou_xyz"]}
        }"#;
        let f = write_tmp(json);
        let checker = AllowlistChecker::from_path(Some(f.path()));
        assert!(checker.is_allowed("lark", "ou_abc"));
        assert!(!checker.is_allowed("lark", "ou_unknown"));
    }

    #[test]
    fn test_allowlist_mode_dingtalk() {
        let json = r#"{
            "dingtalk": {"enabled": true, "mode": "allowlist", "users": ["user_1", "user_2"]}
        }"#;
        let f = write_tmp(json);
        let checker = AllowlistChecker::from_path(Some(f.path()));
        assert!(checker.is_allowed("dingtalk", "user_1"));
        assert!(!checker.is_allowed("dingtalk", "user_99"));
    }

    #[test]
    fn test_disabled_channel_denies_all() {
        let json = r#"{
            "lark": {"enabled": false, "mode": "allowlist", "open_ids": ["ou_abc"]}
        }"#;
        let f = write_tmp(json);
        let checker = AllowlistChecker::from_path(Some(f.path()));
        assert!(!checker.is_allowed("lark", "ou_abc"));
    }
}
