use std::path::{Path, PathBuf};

pub const SOFTWARE: &str = "LIBGL_ALWAYS_SOFTWARE";
pub const EGL_VENDORS: &str = "__EGL_VENDOR_LIBRARY_FILENAMES";
pub const LP_NUM_THREADS: &str = "LP_NUM_THREADS";
const MESA_VENDOR: &str = "50_mesa.json";
const NVIDIA_VENDOR: &str = "10_nvidia.json";
const NVIDIA_DRIVER: &str = "/proc/driver/nvidia/version";
const VENDOR_DIRS: [&str; 4] = [
    "/usr/share/glvnd/egl_vendor.d",
    "/etc/glvnd/egl_vendor.d",
    "/usr/lib/x86_64-linux-gnu/GL/glvnd/egl_vendor.d",
    "/usr/lib/aarch64-linux-gnu/GL/glvnd/egl_vendor.d",
];

const STARTING: &str = "starting";
pub const SETTLED_AFTER: std::time::Duration = std::time::Duration::from_secs(20);
pub const SETTLED_AFTER_PAINT: std::time::Duration = std::time::Duration::from_secs(3);

pub fn settle_deadline(
    current: Option<std::time::Instant>,
    now: std::time::Instant,
    painted: bool,
) -> Option<std::time::Instant> {
    let deadline = current?;
    if painted {
        Some(deadline.min(now + SETTLED_AFTER_PAINT))
    } else {
        Some(deadline)
    }
}

pub fn after_crash(crashed: bool, cpu_rendering: bool) -> Option<bool> {
    crashed.then_some(!cpu_rendering)
}

pub fn starting_marker(storage: &Path) -> PathBuf {
    storage.join(STARTING)
}

pub fn guard(storage: Option<&Path>, cpu_rendering: bool, save: impl FnOnce(bool)) -> bool {
    let Some(dir) = storage else {
        return cpu_rendering;
    };
    let marker = starting_marker(dir);
    let flipped = after_crash(marker.exists(), cpu_rendering);
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&marker, b"");
    match flipped {
        Some(now) => {
            save(now);
            eprintln!(
                "Soundscape didn't start properly last time, so CPU rendering is now {}.",
                if now { "on" } else { "off" }
            );
            now
        }
        None => cpu_rendering,
    }
}

pub fn settled(storage: Option<&Path>) {
    if let Some(dir) = storage {
        let _ = std::fs::remove_file(starting_marker(dir));
    }
}

fn vendor(name: &str, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    VENDOR_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|path| exists(path))
}

pub fn mesa_vendor(exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    vendor(MESA_VENDOR, exists)
}

pub fn nvidia_vendor(exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    if !exists(Path::new(NVIDIA_DRIVER)) {
        return None;
    }
    vendor(NVIDIA_VENDOR, exists)
}

pub fn to_set(
    enabled: bool,
    present: impl Fn(&str) -> bool,
    mesa: Option<PathBuf>,
    nvidia: Option<PathBuf>,
) -> Vec<(&'static str, String)> {
    let mut wanted = Vec::new();
    if enabled {
        wanted.push((SOFTWARE, "1".to_string()));
        wanted.push((LP_NUM_THREADS, "2".to_string()));
        if let Some(path) = mesa {
            wanted.push((EGL_VENDORS, path.to_string_lossy().into_owned()));
        }
    } else if let Some(path) = nvidia {
        wanted.push((EGL_VENDORS, path.to_string_lossy().into_owned()));
    }
    wanted
        .into_iter()
        .filter(|(name, _)| !present(name))
        .collect()
}

pub fn prepare(enabled: bool) -> Vec<&'static str> {
    let wanted = to_set(
        enabled,
        |name| std::env::var_os(name).is_some(),
        mesa_vendor(|path| path.exists()),
        nvidia_vendor(|path| path.exists()),
    );
    let mut set = Vec::new();
    for (name, value) in wanted {
        unsafe { std::env::set_var(name, value) };
        set.push(name);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_mesa_to_draw_with_the_processor() {
        let mesa = Some(PathBuf::from("/usr/share/glvnd/egl_vendor.d/50_mesa.json"));
        assert_eq!(
            to_set(true, |_| false, mesa.clone(), None),
            vec![
                (SOFTWARE, "1".to_string()),
                (LP_NUM_THREADS, "2".to_string()),
                (
                    EGL_VENDORS,
                    "/usr/share/glvnd/egl_vendor.d/50_mesa.json".to_string()
                )
            ]
        );
        assert_eq!(
            to_set(true, |_| false, None, None),
            vec![
                (SOFTWARE, "1".to_string()),
                (LP_NUM_THREADS, "2".to_string()),
            ]
        );
        assert!(to_set(false, |_| false, mesa.clone(), None).is_empty());
        assert_eq!(
            to_set(true, |name| name == EGL_VENDORS, mesa, None),
            vec![
                (SOFTWARE, "1".to_string()),
                (LP_NUM_THREADS, "2".to_string()),
            ]
        );
    }

    #[test]
    fn flips_cpu_rendering_after_a_crashed_start() {
        assert_eq!(after_crash(true, true), Some(false));
        assert_eq!(after_crash(true, false), Some(true));
        assert_eq!(after_crash(false, true), None);
        let dir =
            std::env::temp_dir().join(format!("soundscape-desktop-guard-{}", std::process::id()));
        let saved = std::cell::Cell::new(None);
        assert!(guard(Some(&dir), true, |now| saved.set(Some(now))));
        assert!(starting_marker(&dir).exists());
        assert_eq!(saved.get(), None);
        assert!(!guard(Some(&dir), true, |now| saved.set(Some(now))));
        assert_eq!(saved.get(), Some(false));
        settled(Some(&dir));
        assert!(!starting_marker(&dir).exists());
        assert!(guard(Some(&dir), true, |_| {}));
        settled(Some(&dir));
        let _ = std::fs::remove_dir(&dir);
        assert!(!guard(None, false, |_| {}));
    }

    #[test]
    fn keeps_mesa_out_when_the_nvidia_driver_draws() {
        let nvidia = Some(PathBuf::from(
            "/usr/share/glvnd/egl_vendor.d/10_nvidia.json",
        ));
        let mesa = Some(PathBuf::from("/usr/share/glvnd/egl_vendor.d/50_mesa.json"));
        assert_eq!(
            to_set(false, |_| false, mesa.clone(), nvidia.clone()),
            vec![(
                EGL_VENDORS,
                "/usr/share/glvnd/egl_vendor.d/10_nvidia.json".to_string()
            )]
        );
        assert!(
            to_set(
                false,
                |name| name == EGL_VENDORS,
                mesa.clone(),
                nvidia.clone()
            )
            .is_empty()
        );
        assert_eq!(
            to_set(true, |_| false, mesa, nvidia),
            vec![
                (SOFTWARE, "1".to_string()),
                (LP_NUM_THREADS, "2".to_string()),
                (
                    EGL_VENDORS,
                    "/usr/share/glvnd/egl_vendor.d/50_mesa.json".to_string()
                )
            ]
        );
    }

    #[test]
    fn finds_the_nvidia_vendor_file_only_with_the_driver_loaded() {
        let everything = |_: &Path| true;
        assert_eq!(
            nvidia_vendor(everything),
            Some(PathBuf::from(
                "/usr/share/glvnd/egl_vendor.d/10_nvidia.json"
            ))
        );
        assert_eq!(nvidia_vendor(|path| path != Path::new(NVIDIA_DRIVER)), None);
        assert_eq!(nvidia_vendor(|path| path == Path::new(NVIDIA_DRIVER)), None);
    }

    #[test]
    fn finds_the_mesa_vendor_file() {
        assert_eq!(
            mesa_vendor(|path| path.starts_with("/etc")),
            Some(PathBuf::from("/etc/glvnd/egl_vendor.d/50_mesa.json"))
        );
        assert_eq!(
            mesa_vendor(|path| path.starts_with("/usr/lib/aarch64-linux-gnu")),
            Some(PathBuf::from(
                "/usr/lib/aarch64-linux-gnu/GL/glvnd/egl_vendor.d/50_mesa.json"
            ))
        );
        assert_eq!(mesa_vendor(|_| false), None);
    }

    #[test]
    fn settles_sooner_after_the_first_paint() {
        let start = std::time::Instant::now();
        let initial = Some(start + SETTLED_AFTER);
        assert_eq!(settle_deadline(initial, start, false), initial);
        let after_paint = start + std::time::Duration::from_millis(500);
        let shorter = settle_deadline(initial, after_paint, true).unwrap();
        assert_eq!(shorter, after_paint + SETTLED_AFTER_PAINT);
        let later = after_paint + std::time::Duration::from_secs(1);
        assert_eq!(settle_deadline(Some(shorter), later, true), Some(shorter));
        assert_eq!(settle_deadline(None, later, true), None);
    }
}
