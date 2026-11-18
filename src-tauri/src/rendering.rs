use crate::config::Config;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const CPU_RENDERING: &str = "WEBKIT_SKIA_ENABLE_CPU_RENDERING";
const IDENTIFIER: &str = "io.github.novakr12.SoundscapeDesktop";
const STARTING: &str = "starting";
pub const SETTLED_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

static ACTIVE: AtomicBool = AtomicBool::new(false);
static RECOVERED: AtomicBool = AtomicBool::new(false);

pub fn to_set(enabled: bool, present: impl Fn(&str) -> bool) -> Vec<(&'static str, &'static str)> {
    if !enabled {
        return Vec::new();
    }
    [(CPU_RENDERING, "1")]
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

pub fn after_crash(crashed: bool, cpu_rendering: bool) -> bool {
    crashed && cpu_rendering
}

pub fn prepare(dir: &Path) {
    let mut config = Config::load(dir);
    let marker = dir.join(STARTING);
    if after_crash(marker.exists(), config.cpu_rendering) {
        config.cpu_rendering = false;
        if config.save(dir).is_ok() {
            RECOVERED.store(true, Ordering::Relaxed);
        }
    }
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(&marker, b"");
    }
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

pub fn recovered() -> bool {
    RECOVERED.load(Ordering::Relaxed)
}

pub fn settled(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(STARTING));
}

pub const SMOOTH_WHEEL: &str = r#"(() => {
  if (window.top !== window || window.soundscapeSmoothWheelReady) return;
  window.soundscapeSmoothWheelReady = true;
  const STIFFNESS = 0.024;
  const STEP_MS = 4;
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
        glide = { at: element.scrollTop, speed: 0, target: element.scrollTop, last: 0, frame: 0 };
        glides.set(element, glide);
      }
      if (!glide.frame) {
        glide.at = element.scrollTop;
        glide.target = element.scrollTop;
        glide.speed = 0;
      }
      const target = Math.max(0, Math.min(most, glide.target + step));
      if (target === glide.target && !glide.frame) return;
      event.preventDefault();
      glide.target = target;
      if (glide.frame) return;
      glide.last = performance.now();
      const move = (now) => {
        if (Math.abs(element.scrollTop - glide.at) > 2) {
          glide.frame = 0;
          return;
        }
        let passed = Math.min(now - glide.last, 50);
        glide.last = now;
        while (passed > 0) {
          const step = Math.min(passed, STEP_MS);
          const pull = STIFFNESS * STIFFNESS * (glide.target - glide.at) - 2 * STIFFNESS * glide.speed;
          glide.speed += pull * step;
          glide.at += glide.speed * step;
          passed -= step;
        }
        glide.at = Math.max(0, Math.min(element.scrollHeight - element.clientHeight, glide.at));
        if (Math.abs(glide.target - glide.at) < 0.5 && Math.abs(glide.speed) < 0.01) {
          glide.at = glide.target;
          glide.speed = 0;
        }
        element.scrollTop = glide.at;
        glide.frame = glide.at === glide.target ? 0 : requestAnimationFrame(move);
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
        assert_eq!(to_set(true, |_| false), vec![(CPU_RENDERING, "1")]);
        assert!(to_set(true, |name| name == CPU_RENDERING).is_empty());
        assert!(to_set(false, |_| false).is_empty());
    }

    #[test]
    fn turns_cpu_rendering_off_after_a_crash() {
        assert!(after_crash(true, true));
        assert!(!after_crash(true, false));
        assert!(!after_crash(false, true));
    }

    #[test]
    fn remembers_a_start_until_it_settles() {
        let dir = std::env::temp_dir().join(format!("soundscape-rendering-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = Config {
            cpu_rendering: true,
            ..Config::default()
        };
        config.save(&dir).unwrap();
        std::fs::write(dir.join(STARTING), b"").unwrap();
        prepare(&dir);
        assert!(recovered());
        assert!(!Config::load(&dir).cpu_rendering);
        assert!(dir.join(STARTING).exists());
        settled(&dir);
        assert!(!dir.join(STARTING).exists());
        let _ = std::fs::remove_dir_all(&dir);
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
