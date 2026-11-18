use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const CPU_RENDERING: &str = "WEBKIT_SKIA_ENABLE_CPU_RENDERING";
pub const NO_DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
const IDENTIFIER: &str = "io.github.novakr12.SoundscapeDesktop";

static ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn to_set(enabled: bool, present: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    if !enabled {
        return Vec::new();
    }
    [(CPU_RENDERING, "1"), (NO_DMABUF, "1")]
        .into_iter()
        .filter(|(name, _)| !present(name))
        .collect()
}

pub fn is_on(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some(text) if !text.is_empty() && text != "0")
}

pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join(IDENTIFIER))
}

pub fn prepare(dir: &Path) {
    let config = Config::load(dir);
    for (name, value) in to_set(config.cpu_rendering, |name| {
        std::env::var_os(name).is_some()
    }) {
        std::env::set_var(name, value);
    }
    ACTIVE.store(
        is_on(std::env::var(CPU_RENDERING).ok().as_deref()),
        Ordering::Relaxed,
    );
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn smooth_scrolling(window: &tauri::WebviewWindow, enabled: bool) {
    use webkit2gtk::{SettingsExt, WebViewExt};
    let _ = window.with_webview(move |webview| {
        if let Some(settings) = webview.inner().settings() {
            settings.set_enable_smooth_scrolling(enabled);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_cpu_rendering_on_unless_asked_otherwise() {
        assert_eq!(
            to_set(true, |_| false),
            vec![(CPU_RENDERING, "1"), (NO_DMABUF, "1")]
        );
        assert_eq!(
            to_set(true, |name| name == NO_DMABUF),
            vec![(CPU_RENDERING, "1")]
        );
        assert!(to_set(true, |_| true).is_empty());
        assert!(to_set(false, |_| false).is_empty());
    }

    #[test]
    fn reads_whether_it_is_on() {
        assert!(is_on(Some("1")));
        assert!(is_on(Some(" yes ")));
        assert!(!is_on(Some("0")));
        assert!(!is_on(Some("")));
        assert!(!is_on(None));
    }

    #[test]
    fn finds_the_settings_folder_like_tauri() {
        let dir = config_dir().expect("a home folder");
        assert!(dir.ends_with(IDENTIFIER));
    }
}
