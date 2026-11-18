use std::path::{Path, PathBuf};

pub const SOFTWARE: &str = "LIBGL_ALWAYS_SOFTWARE";
pub const EGL_VENDORS: &str = "__EGL_VENDOR_LIBRARY_FILENAMES";
const MESA_VENDOR: &str = "50_mesa.json";
const VENDOR_DIRS: [&str; 3] = [
    "/usr/share/glvnd/egl_vendor.d",
    "/etc/glvnd/egl_vendor.d",
    "/usr/lib/x86_64-linux-gnu/GL/glvnd/egl_vendor.d",
];

const STARTING: &str = "starting";
pub const SETTLED_AFTER: std::time::Duration = std::time::Duration::from_secs(20);

pub fn after_crash(crashed: bool, cpu_rendering: bool) -> Option<bool> {
    crashed.then_some(!cpu_rendering)
}

pub fn starting_marker(storage: &Path) -> PathBuf {
    storage.join(STARTING)
}

pub fn guard(storage: Option<&Path>, cpu_rendering: bool) -> bool {
    let Some(dir) = storage else {
        return cpu_rendering;
    };
    let marker = starting_marker(dir);
    let flipped = after_crash(marker.exists(), cpu_rendering);
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&marker, b"");
    match flipped {
        Some(now) => {
            let mut own = crate::bridge::load(Some(dir));
            own.cpu_rendering = Some(now);
            crate::bridge::save(Some(dir), &own);
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

pub fn mesa_vendor(exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    VENDOR_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(MESA_VENDOR))
        .find(|path| exists(path))
}

pub fn to_set(
    enabled: bool,
    present: impl Fn(&str) -> bool,
    mesa: Option<PathBuf>,
) -> Vec<(&'static str, String)> {
    if !enabled {
        return Vec::new();
    }
    let mut wanted = vec![(SOFTWARE, "1".to_string())];
    if let Some(path) = mesa {
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
            to_set(true, |_| false, mesa.clone()),
            vec![
                (SOFTWARE, "1".to_string()),
                (
                    EGL_VENDORS,
                    "/usr/share/glvnd/egl_vendor.d/50_mesa.json".to_string()
                )
            ]
        );
        assert_eq!(
            to_set(true, |_| false, None),
            vec![(SOFTWARE, "1".to_string())]
        );
        assert!(to_set(false, |_| false, mesa.clone()).is_empty());
        assert_eq!(
            to_set(true, |name| name == EGL_VENDORS, mesa),
            vec![(SOFTWARE, "1".to_string())]
        );
    }

    #[test]
    fn flips_cpu_rendering_after_a_crashed_start() {
        assert_eq!(after_crash(true, true), Some(false));
        assert_eq!(after_crash(true, false), Some(true));
        assert_eq!(after_crash(false, true), None);
        let dir =
            std::env::temp_dir().join(format!("soundscape-servo-guard-{}", std::process::id()));
        assert!(guard(Some(&dir), true));
        assert!(starting_marker(&dir).exists());
        assert!(!guard(Some(&dir), true));
        assert_eq!(crate::bridge::load(Some(&dir)).cpu_rendering, Some(false));
        settled(Some(&dir));
        assert!(!starting_marker(&dir).exists());
        assert!(guard(Some(&dir), true));
        settled(Some(&dir));
        let _ = std::fs::remove_file(crate::bridge::own_file(&dir));
        let _ = std::fs::remove_dir(&dir);
        assert!(!guard(None, false));
    }

    #[test]
    fn finds_the_mesa_vendor_file() {
        assert_eq!(
            mesa_vendor(|path| path.starts_with("/etc")),
            Some(PathBuf::from("/etc/glvnd/egl_vendor.d/50_mesa.json"))
        );
        assert_eq!(mesa_vendor(|_| false), None);
    }
}
