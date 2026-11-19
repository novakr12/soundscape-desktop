use servo::Theme;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

pub fn from_scheme(scheme: u32) -> Theme {
    if scheme == 1 {
        Theme::Dark
    } else {
        Theme::Light
    }
}

pub fn chosen(value: Option<&str>) -> Option<Theme> {
    match value.map(str::trim) {
        Some("dark") => Some(Theme::Dark),
        Some("light") => Some(Theme::Light),
        _ => None,
    }
}

pub fn scheme(value: &Value) -> Option<u32> {
    match value {
        Value::U32(scheme) => Some(*scheme),
        Value::Value(inner) => scheme(inner),
        _ => None,
    }
}

pub fn backdrop(theme: Theme) -> [f64; 4] {
    match theme {
        Theme::Dark => [17.0 / 255.0, 17.0 / 255.0, 17.0 / 255.0, 1.0],
        Theme::Light => [1.0, 1.0, 1.0, 1.0],
    }
}

fn read(proxy: &Proxy) -> Option<Theme> {
    let current = proxy
        .call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY))
        .or_else(|_| proxy.call::<_, _, OwnedValue>("Read", &(NAMESPACE, KEY)));
    current.ok().as_deref().and_then(scheme).map(from_scheme)
}

pub fn current() -> Option<Theme> {
    let connection = Connection::session().ok()?;
    let proxy = Proxy::new(&connection, DESTINATION, PATH, INTERFACE).ok()?;
    read(&proxy)
}

pub fn watch(send: impl Fn(Theme) + Send + 'static) {
    std::thread::spawn(move || {
        let Ok(connection) = Connection::session() else {
            return;
        };
        let Ok(proxy) = Proxy::new(&connection, DESTINATION, PATH, INTERFACE) else {
            return;
        };
        if let Some(found) = read(&proxy) {
            send(found);
        }
        let Ok(changes) = proxy.receive_signal("SettingChanged") else {
            return;
        };
        for change in changes {
            let Ok((namespace, key, value)) =
                change.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            if namespace == NAMESPACE
                && key == KEY
                && let Some(found) = scheme(&value)
            {
                send(from_scheme(found));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_desktop_colour_scheme() {
        assert_eq!(from_scheme(1), Theme::Dark);
        assert_eq!(from_scheme(2), Theme::Light);
        assert_eq!(from_scheme(0), Theme::Light);
        assert_eq!(scheme(&Value::U32(1)), Some(1));
        assert_eq!(scheme(&Value::Value(Box::new(Value::U32(2)))), Some(2));
        assert_eq!(scheme(&Value::Str("dark".into())), None);
        assert_eq!(chosen(Some("dark")), Some(Theme::Dark));
        assert_eq!(chosen(Some("light")), Some(Theme::Light));
        assert_eq!(chosen(Some("blue")), None);
        assert_eq!(backdrop(Theme::Light), [1.0, 1.0, 1.0, 1.0]);
        assert!(backdrop(Theme::Dark)[0] < 0.1);
    }
}
