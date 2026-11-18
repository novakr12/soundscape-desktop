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

pub fn settings_script(version: &str, smooth_scrolling: bool) -> String {
    let detail = serde_json::json!({
        "settings": {
            "app": "servo",
            "version": version,
            "smooth_scrolling": smooth_scrolling,
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
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "do")]
    action: String,
    name: Option<String>,
    value: Option<bool>,
}

pub fn commands(json: &str) -> Vec<Command> {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value::<Raw>(value).ok())
        .filter_map(
            |raw| match (raw.action.as_str(), raw.name.as_deref(), raw.value) {
                ("option", Some("smooth_scrolling"), Some(value)) => {
                    Some(Command::SmoothScrolling(value))
                }
                _ => None,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_the_page_its_settings() {
        let script = settings_script("0.1.0", false);
        assert!(script.contains(r#""app":"servo""#));
        assert!(script.contains(r#""smooth_scrolling":false"#));
        assert!(script.contains(r#""version":"0.1.0""#));
        assert!(script.contains("soundscape-app-settings"));
    }

    #[test]
    fn reads_the_page_commands() {
        assert_eq!(
            commands(
                r#"[{"do":"option","name":"smooth_scrolling","value":false},{"do":"send-now"},{"do":"option","name":"discord","value":true},5]"#
            ),
            vec![Command::SmoothScrolling(false)]
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
            },
        );
        assert_eq!(load(Some(&dir)).smooth_scrolling, Some(false));
        let _ = std::fs::remove_file(own_file(&dir));
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(load(None), Own::default());
    }
}
