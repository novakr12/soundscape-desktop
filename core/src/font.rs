use crate::host::async_runtime;
use crate::theme::{DESTINATION, INTERFACE, PATH};
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use tokio::sync::watch;
use zbus::zvariant::{OwnedValue, Value};
use zbus::Proxy;

const GNOME: (&str, &str) = ("org.gnome.desktop.interface", "font-name");
const KDE: (&str, &str) = ("org.kde.kdeglobals.General", "font");

static CACHED: RwLock<Option<Option<String>>> = RwLock::new(None);
static SENDER: OnceLock<watch::Sender<Option<Option<String>>>> = OnceLock::new();

pub(crate) fn is_font_setting(namespace: &str, key: &str) -> bool {
    (namespace, key) == GNOME || (namespace, key) == KDE
}

pub fn from_gnome(setting: &str) -> Option<String> {
    let setting = setting.trim().trim_matches('\'');
    let family = match setting.rsplit_once(' ') {
        Some((family, size)) if size.parse::<f64>().is_ok() => family,
        _ => setting,
    };
    clean(family)
}

pub fn from_kde(setting: &str) -> Option<String> {
    clean(setting.split(',').next()?)
}

fn clean(family: &str) -> Option<String> {
    let family = family.trim();
    (!family.is_empty() && !family.eq_ignore_ascii_case("sans-serif")).then(|| family.to_string())
}

pub fn from_kdeglobals(text: &str) -> Option<String> {
    let mut general = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            general = line == "[General]";
        } else if general {
            if let Some(value) = line.strip_prefix("font=") {
                return from_kde(value);
            }
        }
    }
    None
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.as_str().to_string()),
        Value::Value(inner) => text(inner),
        _ => None,
    }
}

async fn read(proxy: &Proxy<'_>, (namespace, key): (&str, &str)) -> Option<String> {
    let current = match proxy
        .call::<_, _, OwnedValue>("ReadOne", &(namespace, key))
        .await
    {
        Ok(value) => Ok(value),
        Err(_) => {
            proxy
                .call::<_, _, OwnedValue>("Read", &(namespace, key))
                .await
        }
    };
    current.ok().as_deref().and_then(text)
}

async fn from_proxy(proxy: &Proxy<'_>) -> Option<String> {
    if let Some(found) = read(proxy, GNOME).await.as_deref().and_then(from_gnome) {
        return Some(found);
    }
    read(proxy, KDE).await.as_deref().and_then(from_kde)
}

async fn from_portal() -> Option<String> {
    let connection = crate::bus::session().await.ok()?;
    let proxy = Proxy::new(&connection, DESTINATION, PATH, INTERFACE)
        .await
        .ok()?;
    from_proxy(&proxy).await
}

fn from_kde_file() -> Option<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    from_kdeglobals(&std::fs::read_to_string(config.join("kdeglobals")).ok()?)
}

fn env_override() -> Option<String> {
    std::env::var("SOUNDSCAPE_FONT")
        .ok()
        .and_then(|family| clean(&family))
}

fn sender() -> &'static watch::Sender<Option<Option<String>>> {
    SENDER.get_or_init(|| {
        let (tx, _) = watch::channel(None);
        tx
    })
}

fn store(family: Option<String>) {
    if let Ok(mut guard) = CACHED.write() {
        *guard = Some(family.clone());
    }
    if let Some(tx) = SENDER.get() {
        let effective = env_override().or(family);
        tx.send_if_modified(|current| {
            if *current == Some(effective.clone()) {
                false
            } else {
                *current = Some(effective);
                true
            }
        });
    }
}

pub(crate) async fn refresh_from_portal(proxy: &Proxy<'_>) {
    let found = match from_proxy(proxy).await {
        Some(family) => Some(family),
        None => from_kde_file(),
    };
    store(found);
}

pub async fn desktop_async() -> Option<String> {
    if let Some(family) = env_override() {
        return Some(family);
    }
    if let Ok(guard) = CACHED.read() {
        if let Some(known) = guard.as_ref() {
            return known.clone();
        }
    }
    let found = match from_portal().await {
        Some(family) => Some(family),
        None => from_kde_file(),
    };
    store(found.clone());
    crate::theme::ensure_portal_watcher();
    found
}

pub fn desktop() -> Option<String> {
    if let Some(family) = env_override() {
        return Some(family);
    }
    if let Ok(guard) = CACHED.read() {
        if let Some(known) = guard.as_ref() {
            return known.clone();
        }
    }
    async_runtime::block_on(desktop_async())
}

pub fn subscribe() -> watch::Receiver<Option<Option<String>>> {
    let rx = sender().subscribe();
    crate::theme::ensure_portal_watcher();
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_gnome_interface_font() {
        assert_eq!(from_gnome("'Adwaita Sans 11'"), Some("Adwaita Sans".into()));
        assert_eq!(from_gnome("Cantarell 10.5"), Some("Cantarell".into()));
        assert_eq!(from_gnome("Inter"), Some("Inter".into()));
        assert_eq!(from_gnome(" "), None);
        assert_eq!(from_gnome("Sans 11"), Some("Sans".into()));
    }

    #[test]
    fn reads_the_kde_interface_font() {
        assert_eq!(
            from_kde("Noto Sans,10,-1,5,400,0,0,0,0,0,0,0,0,0,0,1"),
            Some("Noto Sans".into())
        );
        let globals = "[Colors]\nfont=Ignored,9\n[General]\nfont=Inter Variable,10,-1\n";
        assert_eq!(from_kdeglobals(globals), Some("Inter Variable".into()));
        assert_eq!(from_kdeglobals("[General]\nColorScheme=Breeze\n"), None);
    }
}
