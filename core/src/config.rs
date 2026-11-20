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
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub notifications: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub autostart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_notification: Option<i64>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub weekly_recap: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recap: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub players: BTreeMap<String, bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<crate::fixes::Fix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_until: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub streak_reminder: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_streak_reminder: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub yearscape_notification: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_yearscape: Option<i32>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub update_notice: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told_update: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub discord: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_app_id: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub native_player: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_size: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub maximized: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub cpu_rendering: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub smooth_scrolling: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_window: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub faster_scripts: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

pub const PAUSE_SECONDS: i64 = 60 * 60;

pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Config {
    pub fn paused_minutes(&self, now: i64) -> Option<u64> {
        self.paused_until
            .map(|until| until - now)
            .filter(|left| *left > 0)
            .map(|left| (left as u64).div_ceil(60))
    }

    pub fn sending(&self, now: i64) -> bool {
        self.scrobbling && self.connected() && self.paused_minutes(now).is_none()
    }
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
            notifications: true,
            autostart: false,
            last_notification: None,
            weekly_recap: true,
            last_recap: None,
            players: BTreeMap::new(),
            fixes: Vec::new(),
            paused_until: None,
            streak_reminder: false,
            last_streak_reminder: None,
            yearscape_notification: true,
            last_yearscape: None,
            update_notice: true,
            told_update: None,
            discord: false,
            discord_app_id: None,
            native_player: true,
            device_id: None,
            window_size: None,
            maximized: false,
            cpu_rendering: false,
            smooth_scrolling: true,
            memory_limit: None,
            keep_window: None,
            faster_scripts: false,
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
    fn a_pause_ends_by_itself() {
        let config = Config {
            server: Some("https://a.example/".into()),
            token: Some("t".into()),
            paused_until: Some(10_000),
            ..Config::default()
        };
        assert_eq!(config.paused_minutes(10_000 - 3600), Some(60));
        assert_eq!(config.paused_minutes(10_000 - 61), Some(2));
        assert_eq!(config.paused_minutes(10_000 - 1), Some(1));
        assert!(!config.sending(10_000 - 1));
        assert_eq!(config.paused_minutes(10_000), None);
        assert!(config.sending(10_000));
        assert!(!Config {
            scrobbling: false,
            ..config
        }
        .sending(20_000));
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
        assert!(config.notifications && !config.autostart && config.last_notification.is_none());
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
