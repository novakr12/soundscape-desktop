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

pub const SMOOTH_WHEEL: &str = r#"(() => {
  if (window.top !== window || window.soundscapeSmoothWheelReady) return;
  window.soundscapeSmoothWheelReady = true;
  const DURATION = 140;
  const LINE = 40;
  const glides = new WeakMap();
  const isWheel = (event) =>
    event.deltaMode !== 0 ||
    (event.wheelDeltaY !== 0 && event.wheelDeltaY % 120 === 0) ||
    (Number.isInteger(event.deltaY) && Math.abs(event.deltaY) >= 40);
  const canScroll = (element, down) => {
    const style = getComputedStyle(element);
    if (!/(auto|scroll)/.test(style.overflowY)) return false;
    if (element.scrollHeight <= element.clientHeight) return false;
    return down ? element.scrollTop + element.clientHeight < element.scrollHeight - 1 : element.scrollTop > 0;
  };
  const scrollerFor = (target, down) => {
    for (let element = target instanceof Element ? target : null; element && element !== document.body; element = element.parentElement) {
      if (element === document.documentElement) break;
      if (canScroll(element, down)) return element;
    }
    return document.scrollingElement;
  };
  addEventListener(
    "wheel",
    (event) => {
      if (window.soundscapeSmoothWheel === false) return;
      if (event.defaultPrevented || event.ctrlKey || event.shiftKey || event.deltaX !== 0 || event.deltaY === 0) return;
      if (!isWheel(event)) return;
      const element = scrollerFor(event.target, event.deltaY > 0);
      if (!element) return;
      const step = event.deltaMode === 1 ? event.deltaY * LINE : event.deltaMode === 2 ? event.deltaY * element.clientHeight : event.deltaY;
      const most = element.scrollHeight - element.clientHeight;
      let glide = glides.get(element);
      if (!glide) {
        glide = { from: 0, to: element.scrollTop, start: 0, frame: 0 };
        glides.set(element, glide);
      }
      const base = glide.frame ? glide.to : element.scrollTop;
      const to = Math.max(0, Math.min(most, base + step));
      if (to === element.scrollTop && !glide.frame) return;
      event.preventDefault();
      glide.from = element.scrollTop;
      glide.to = to;
      glide.start = performance.now();
      if (glide.frame) return;
      const move = (now) => {
        const done = Math.min(1, (now - glide.start) / DURATION);
        const eased = 1 - (1 - done) ** 3;
        element.scrollTop = glide.from + (glide.to - glide.from) * eased;
        glide.frame = done < 1 ? requestAnimationFrame(move) : 0;
      };
      glide.frame = requestAnimationFrame(move);
    },
    { passive: false },
  );
})();"#;

pub fn wheel_switch(enabled: bool) -> String {
    format!("window.soundscapeSmoothWheel = {enabled};")
}

pub fn smooth_scrolling(window: &tauri::WebviewWindow, enabled: bool) {
    use webkit2gtk::{SettingsExt, WebViewExt};
    let _ = window.with_webview(|webview| {
        if let Some(settings) = webview.inner().settings() {
            settings.set_enable_smooth_scrolling(false);
        }
    });
    let _ = window.eval(wheel_switch(enabled));
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
    fn glides_the_mouse_wheel_only_when_switched_on() {
        assert!(SMOOTH_WHEEL.contains("passive: false"));
        assert!(SMOOTH_WHEEL.contains("window.soundscapeSmoothWheel === false"));
        assert!(SMOOTH_WHEEL.contains("event.ctrlKey"));
        assert_eq!(wheel_switch(false), "window.soundscapeSmoothWheel = false;");
        assert_eq!(wheel_switch(true), "window.soundscapeSmoothWheel = true;");
    }

    #[test]
    fn finds_the_settings_folder_like_tauri() {
        let dir = config_dir().expect("a home folder");
        assert!(dir.ends_with(IDENTIFIER));
    }
}
