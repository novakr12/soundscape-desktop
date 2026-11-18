use serde::Deserialize;
use std::path::PathBuf;
use url::Url;

const IDENTIFIER: &str = "io.github.novakr12.SoundscapeDesktop";

#[derive(Deserialize)]
struct Saved {
    server: Option<String>,
    #[serde(default = "yes")]
    smooth_scrolling: bool,
}

fn yes() -> bool {
    true
}

pub struct Profile {
    pub server: Option<Url>,
    pub smooth_scrolling: bool,
    pub storage: Option<PathBuf>,
    pub show_fps: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn config_files(config_home: Option<PathBuf>, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(dir) = config_home {
        files.push(dir.join(IDENTIFIER).join("config.json"));
    }
    if let Some(home) = home {
        files.push(
            home.join(".var/app")
                .join(IDENTIFIER)
                .join("config")
                .join(IDENTIFIER)
                .join("config.json"),
        );
        files.push(home.join(".config").join(IDENTIFIER).join("config.json"));
    }
    files
}

pub fn server_from(given: Option<&str>, saved: Option<&str>) -> Option<Url> {
    given
        .or(saved)
        .and_then(|text| Url::parse(text.trim()).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
}

pub fn storage_dir(data_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    data_home
        .or_else(|| home.map(|home| home.join(".local/share")))
        .map(|dir| dir.join("soundscape-servo"))
}

pub fn wants(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes"))
}

fn saved(files: &[PathBuf]) -> Option<Saved> {
    files
        .iter()
        .find_map(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
}

pub fn load() -> Profile {
    let files = config_files(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        home(),
    );
    let saved = saved(&files);
    let given = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("SOUNDSCAPE_SERVER").ok());
    Profile {
        server: server_from(
            given.as_deref(),
            saved.as_ref().and_then(|saved| saved.server.as_deref()),
        ),
        smooth_scrolling: saved.map(|saved| saved.smooth_scrolling).unwrap_or(true),
        storage: storage_dir(std::env::var_os("XDG_DATA_HOME").map(PathBuf::from), home()),
        show_fps: wants(std::env::var("SOUNDSCAPE_SHOW_FPS").ok().as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_where_the_desktop_app_keeps_its_settings() {
        let files = config_files(Some("/c".into()), Some("/h".into()));
        assert_eq!(
            files,
            vec![
                PathBuf::from("/c/io.github.novakr12.SoundscapeDesktop/config.json"),
                PathBuf::from(
                    "/h/.var/app/io.github.novakr12.SoundscapeDesktop/config/io.github.novakr12.SoundscapeDesktop/config.json"
                ),
                PathBuf::from("/h/.config/io.github.novakr12.SoundscapeDesktop/config.json"),
            ]
        );
    }

    #[test]
    fn prefers_the_given_server() {
        assert_eq!(
            server_from(Some(" https://a.example/ "), Some("https://b.example/"))
                .unwrap()
                .as_str(),
            "https://a.example/"
        );
        assert_eq!(
            server_from(None, Some("http://b.example:3000"))
                .unwrap()
                .as_str(),
            "http://b.example:3000/"
        );
        assert!(server_from(Some("file:///etc"), None).is_none());
        assert!(server_from(None, None).is_none());
    }

    #[test]
    fn reads_the_saved_settings() {
        let saved: Saved =
            serde_json::from_str(r#"{"server":"https://a.example","token":"x"}"#).unwrap();
        assert!(saved.smooth_scrolling);
        let saved: Saved = serde_json::from_str(r#"{"smooth_scrolling":false}"#).unwrap();
        assert!(!saved.smooth_scrolling);
        assert!(saved.server.is_none());
    }

    #[test]
    fn keeps_its_own_storage() {
        assert_eq!(
            storage_dir(None, Some("/h".into())),
            Some(PathBuf::from("/h/.local/share/soundscape-servo"))
        );
        assert_eq!(
            storage_dir(Some("/d".into()), Some("/h".into())),
            Some(PathBuf::from("/d/soundscape-servo"))
        );
        assert!(wants(Some("1")));
        assert!(!wants(Some("0")));
        assert!(!wants(None));
    }
}
