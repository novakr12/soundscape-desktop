use crate::host::async_runtime;
use futures_util::StreamExt;
use std::sync::OnceLock;
use tokio::sync::watch;
use zbus::zvariant::{OwnedValue, Value};
use zbus::Proxy;

pub use soundscape_protocol::theme::{chosen_dark, from_scheme_dark};

pub(crate) const DESTINATION: &str = "org.freedesktop.portal.Desktop";
pub(crate) const PATH: &str = "/org/freedesktop/portal/desktop";
pub(crate) const INTERFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

static SENDER: OnceLock<watch::Sender<Option<bool>>> = OnceLock::new();
static WATCHER: OnceLock<()> = OnceLock::new();

pub fn scheme(value: &Value) -> Option<u32> {
    match value {
        Value::U32(scheme) => Some(*scheme),
        Value::Value(inner) => scheme(inner),
        _ => None,
    }
}

async fn read(proxy: &Proxy<'_>) -> Option<bool> {
    let current = match proxy
        .call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY))
        .await
    {
        Ok(value) => Ok(value),
        Err(_) => {
            proxy
                .call::<_, _, OwnedValue>("Read", &(NAMESPACE, KEY))
                .await
        }
    };
    current
        .ok()
        .as_deref()
        .and_then(scheme)
        .map(from_scheme_dark)
}

pub async fn current_dark() -> Option<bool> {
    if let Some(dark) = chosen_dark(std::env::var("SOUNDSCAPE_THEME").ok().as_deref()) {
        return Some(dark);
    }
    if let Some(sender) = SENDER.get() {
        if let Some(known) = *sender.borrow() {
            return Some(known);
        }
    }
    let connection = crate::bus::session().await.ok()?;
    let proxy = Proxy::new(&connection, DESTINATION, PATH, INTERFACE)
        .await
        .ok()?;
    let found = read(&proxy).await?;
    publish(found);
    Some(found)
}

fn sender() -> &'static watch::Sender<Option<bool>> {
    SENDER.get_or_init(|| {
        let initial = chosen_dark(std::env::var("SOUNDSCAPE_THEME").ok().as_deref());
        let (tx, _) = watch::channel(initial);
        tx
    })
}

fn publish(dark: bool) {
    if chosen_dark(std::env::var("SOUNDSCAPE_THEME").ok().as_deref()).is_some() {
        return;
    }
    sender().send_if_modified(|current| {
        if *current == Some(dark) {
            false
        } else {
            *current = Some(dark);
            true
        }
    });
}

pub(crate) fn ensure_portal_watcher() {
    WATCHER.get_or_init(|| {
        async_runtime::spawn(async {
            let Ok(connection) = crate::bus::session().await else {
                return;
            };
            let Ok(proxy) = Proxy::new(&connection, DESTINATION, PATH, INTERFACE).await else {
                return;
            };
            if let Some(found) = read(&proxy).await {
                publish(found);
            }
            let Ok(mut changes) = proxy.receive_signal("SettingChanged").await else {
                return;
            };
            while let Some(change) = changes.next().await {
                let Ok((namespace, key, value)) =
                    change.body().deserialize::<(String, String, OwnedValue)>()
                else {
                    continue;
                };
                if namespace == NAMESPACE && key == KEY {
                    if let Some(found) = scheme(&value) {
                        publish(from_scheme_dark(found));
                    }
                } else if crate::font::is_font_setting(&namespace, &key) {
                    crate::font::refresh_from_portal(&proxy).await;
                }
            }
        });
    });
}

pub fn subscribe() -> watch::Receiver<Option<bool>> {
    let rx = sender().subscribe();
    ensure_portal_watcher();
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_portal_color_scheme() {
        assert!(from_scheme_dark(1));
        assert!(!from_scheme_dark(2));
        assert!(!from_scheme_dark(0));
        assert_eq!(chosen_dark(Some("dark")), Some(true));
        assert_eq!(chosen_dark(Some(" light ")), Some(false));
        assert_eq!(chosen_dark(Some("auto")), None);
        assert_eq!(scheme(&Value::U32(1)), Some(1));
        assert_eq!(scheme(&Value::Value(Box::new(Value::U32(2)))), Some(2));
        assert_eq!(scheme(&Value::from("dark")), None);
    }
}
