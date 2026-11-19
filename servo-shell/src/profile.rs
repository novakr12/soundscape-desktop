use soundscape_desktop::config::Config;
use std::path::PathBuf;
use url::Url;

pub struct Profile {
    pub server: Option<Url>,
    pub cpu_rendering: bool,
    pub faster_scripts: bool,
    pub memory_limit: u32,
    pub theme: servo::Theme,
    pub smooth_scrolling: bool,
    pub storage: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub show_fps: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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

pub fn given_server() -> Option<String> {
    std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with("--"))
        .or_else(|| std::env::var("SOUNDSCAPE_SERVER").ok())
}

pub fn load() -> Profile {
    let config_dir = soundscape_desktop::config_dir();
    let saved = config_dir.as_deref().map(Config::load).unwrap_or_default();
    let storage = storage_dir(std::env::var_os("XDG_DATA_HOME").map(PathBuf::from), home());
    Profile {
        server: server_from(given_server().as_deref(), saved.server.as_deref()),
        cpu_rendering: saved.cpu_rendering,
        faster_scripts: saved.faster_scripts,
        memory_limit: soundscape_desktop::allowed_memory_limit(
            saved
                .memory_limit
                .unwrap_or(soundscape_desktop::MEMORY_LIMIT_MB),
        ),
        theme: crate::theme::chosen(std::env::var("SOUNDSCAPE_THEME").ok().as_deref())
            .or_else(crate::theme::current)
            .unwrap_or(servo::Theme::Light),
        smooth_scrolling: saved.smooth_scrolling,
        storage,
        config: config_dir,
        show_fps: wants(std::env::var("SOUNDSCAPE_SHOW_FPS").ok().as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
