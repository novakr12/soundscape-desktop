use soundscape_protocol::Start;
use std::path::PathBuf;

pub use soundscape_protocol::wants;

pub struct Profile {
    pub cpu_rendering: bool,
    pub faster_scripts: bool,
    pub theme: servo::Theme,
    pub storage: Option<PathBuf>,
    pub show_fps: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

const OLD_STORAGE: &str = "soundscape-servo";

fn data_dir(data_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    data_home
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".local/share")))
}

pub fn storage_dir(data_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    data_dir(data_home, home).map(|dir| dir.join(soundscape_protocol::IDENTIFIER).join("servo"))
}

pub fn move_old_storage(data_home: Option<PathBuf>, home: Option<PathBuf>) {
    let (Some(old), Some(new)) = (
        data_dir(data_home.clone(), home.clone()).map(|dir| dir.join(OLD_STORAGE)),
        storage_dir(data_home, home),
    ) else {
        return;
    };
    if !old.is_dir() || new.exists() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::rename(&old, &new);
}

pub fn from_start(start: &Start) -> Profile {
    let data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    move_old_storage(data_home.clone(), home());
    let storage = storage_dir(data_home, home());
    Profile {
        cpu_rendering: start.cpu_rendering,
        faster_scripts: start.faster_scripts,
        theme: crate::theme::chosen(std::env::var("SOUNDSCAPE_THEME").ok().as_deref())
            .unwrap_or_else(|| crate::theme::from_dark(start.dark)),
        storage,
        show_fps: wants(std::env::var("SOUNDSCAPE_SHOW_FPS").ok().as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundscape_protocol::server_from;

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
            Some(PathBuf::from(
                "/h/.local/share/io.github.novakr12.SoundscapeDesktop/servo"
            ))
        );
        assert_eq!(
            storage_dir(Some("/d".into()), Some("/h".into())),
            Some(PathBuf::from(
                "/d/io.github.novakr12.SoundscapeDesktop/servo"
            ))
        );
        assert_eq!(
            storage_dir(Some("relative".into()), Some("/h".into())),
            Some(PathBuf::from(
                "/h/.local/share/io.github.novakr12.SoundscapeDesktop/servo"
            ))
        );
        assert!(wants(Some("1")));
        assert!(!wants(Some("0")));
        assert!(!wants(None));
    }

    #[test]
    fn moves_the_storage_of_earlier_builds_once() {
        let data = std::env::temp_dir().join(format!("soundscape-storage-{}", std::process::id()));
        let old = data.join(OLD_STORAGE);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("cookies"), "kept").unwrap();
        move_old_storage(Some(data.clone()), None);
        let new = storage_dir(Some(data.clone()), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(new.join("cookies")).unwrap(),
            "kept"
        );
        assert!(!old.exists());
        std::fs::create_dir_all(&old).unwrap();
        move_old_storage(Some(data.clone()), None);
        assert!(old.exists());
        std::fs::remove_dir_all(data).unwrap();
    }
}
