use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub scrobbling: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub keep_running: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub told_about_background: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub players: BTreeMap<String, bool>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            server: None,
            user: None,
            token: None,
            scrobbling: true,
            keep_running: true,
            told_about_background: false,
            players: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn connected(&self) -> bool {
        self.server.is_some() && self.token.is_some()
    }

    pub fn file(dir: &Path) -> PathBuf {
        dir.join("config.json")
    }

    pub fn load(dir: &Path) -> Config {
        fs::read_to_string(Self::file(dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let path = Self::file(dir);
        let temporary = dir.join("config.json.new");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "soundscape-desktop-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_or_broken_files_mean_not_set_up() {
        let dir = scratch("missing");
        assert_eq!(Config::load(&dir), Config::default());
        fs::create_dir_all(&dir).unwrap();
        fs::write(Config::file(&dir), "not json").unwrap();
        assert_eq!(Config::load(&dir), Config::default());
        assert!(!Config::default().connected());
    }

    #[test]
    fn older_files_keep_scrobbling_on() {
        let dir = scratch("older");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Config::file(&dir),
            r#"{"server":"https://a.example","user":"u","token":"t"}"#,
        )
        .unwrap();
        let config = Config::load(&dir);
        assert!(config.scrobbling && config.keep_running && !config.told_about_background);
        assert!(config.players.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_privately_and_reads_back() {
        let dir = scratch("roundtrip");
        let mut config = Config {
            server: Some("https://music.example.com".into()),
            user: Some("listener".into()),
            token: Some("secret".into()),
            ..Config::default()
        };
        config.players.insert("firefox".into(), true);
        config.scrobbling = false;
        config.save(&dir).unwrap();
        assert_eq!(Config::load(&dir), config);
        assert!(config.connected());
        let mode = fs::metadata(Config::file(&dir))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&dir);
    }
}
