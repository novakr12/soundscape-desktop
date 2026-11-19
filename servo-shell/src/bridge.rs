use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const TAKE_COMMANDS: &str = r#"(() => {
  const waiting = Array.isArray(window.soundscapeAppCommands) ? window.soundscapeAppCommands : [];
  window.soundscapeAppCommands = [];
  return JSON.stringify(waiting);
})()"#;

const OWN_FILE: &str = "settings.json";

#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Own {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth_scrolling: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_rendering: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<u32>,
}

pub fn own_file(storage: &Path) -> PathBuf {
    storage.join(OWN_FILE)
}

pub fn load(storage: Option<&Path>) -> Own {
    storage
        .and_then(|dir| std::fs::read_to_string(own_file(dir)).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(storage: Option<&Path>, own: &Own) {
    let Some(dir) = storage else {
        return;
    };
    let _ = std::fs::create_dir_all(dir);
    if let Ok(text) = serde_json::to_string_pretty(own) {
        let _ = std::fs::write(own_file(dir), text);
    }
}

pub struct Shown<'a> {
    pub version: &'a str,
    pub smooth_scrolling: bool,
    pub cpu_rendering: bool,
    pub cpu_rendering_now: bool,
    pub memory_limit: u32,
}

pub fn settings_script(shown: &Shown) -> String {
    let detail = serde_json::json!({
        "settings": {
            "app": "servo",
            "version": shown.version,
            "smooth_scrolling": shown.smooth_scrolling,
            "cpu_rendering": shown.cpu_rendering,
            "cpu_rendering_now": shown.cpu_rendering_now,
            "memory_limit": shown.memory_limit,
        },
        "error": null,
    });
    format!(
        r#"(() => {{
  const detail = {detail};
  window.soundscapeAppSettings = detail;
  window.dispatchEvent(new CustomEvent("soundscape-app-settings", {{ detail }}));
}})()"#
    )
}

#[derive(Debug, PartialEq)]
pub enum Command {
    SmoothScrolling(bool),
    CpuRendering(bool),
    MemoryLimit(u32),
    Restart,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "do")]
    action: String,
    name: Option<String>,
    value: Option<bool>,
    mb: Option<u32>,
}

pub fn commands(json: &str) -> Vec<Command> {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value::<Raw>(value).ok())
        .filter_map(|raw| {
            if raw.action == "memory-limit" {
                return raw.mb.map(Command::MemoryLimit);
            }
            match (raw.action.as_str(), raw.name.as_deref(), raw.value) {
                ("option", Some("smooth_scrolling"), Some(value)) => {
                    Some(Command::SmoothScrolling(value))
                }
                ("option", Some("cpu_rendering"), Some(value)) => {
                    Some(Command::CpuRendering(value))
                }
                ("restart", _, _) => Some(Command::Restart),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_the_page_its_settings() {
        let script = settings_script(&Shown {
            version: "0.1.0",
            smooth_scrolling: false,
            cpu_rendering: true,
            cpu_rendering_now: false,
            memory_limit: 500,
        });
        assert!(script.contains(r#""memory_limit":500"#));
        assert!(script.contains(r#""cpu_rendering":true"#));
        assert!(script.contains(r#""cpu_rendering_now":false"#));
        assert!(script.contains(r#""app":"servo""#));
        assert!(script.contains(r#""smooth_scrolling":false"#));
        assert!(script.contains(r#""version":"0.1.0""#));
        assert!(script.contains("soundscape-app-settings"));
    }

    #[test]
    fn reads_the_page_commands() {
        assert_eq!(
            commands(
                r#"[{"do":"option","name":"smooth_scrolling","value":false},{"do":"send-now"},{"do":"option","name":"discord","value":true},5,{"do":"option","name":"cpu_rendering","value":true},{"do":"restart"},{"do":"memory-limit","mb":700},{"do":"memory-limit"}]"#
            ),
            vec![
                Command::SmoothScrolling(false),
                Command::CpuRendering(true),
                Command::Restart,
                Command::MemoryLimit(700)
            ]
        );
        assert!(commands("nonsense").is_empty());
        assert!(commands("[]").is_empty());
    }

    #[test]
    fn keeps_its_own_settings() {
        let dir =
            std::env::temp_dir().join(format!("soundscape-servo-test-{}", std::process::id()));
        assert_eq!(load(Some(&dir)), Own::default());
        save(
            Some(&dir),
            &Own {
                smooth_scrolling: Some(false),
                cpu_rendering: Some(true),
                memory_limit: Some(0),
            },
        );
        assert_eq!(load(Some(&dir)).smooth_scrolling, Some(false));
        assert_eq!(load(Some(&dir)).cpu_rendering, Some(true));
        assert_eq!(load(Some(&dir)).memory_limit, Some(0));
        let _ = std::fs::remove_file(own_file(&dir));
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(load(None), Own::default());
    }
}
