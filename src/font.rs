use std::path::PathBuf;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Settings";
const GNOME: (&str, &str) = ("org.gnome.desktop.interface", "font-name");
const KDE: (&str, &str) = ("org.kde.kdeglobals.General", "font");

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
        } else if general && let Some(value) = line.strip_prefix("font=") {
            return from_kde(value);
        }
    }
    None
}

pub fn stylesheet(family: &str) -> String {
    let quoted: String = family
        .chars()
        .filter(|letter| !letter.is_control())
        .flat_map(|letter| match letter {
            '"' | '\\' => vec!['\\', letter],
            _ => vec![letter],
        })
        .collect();
    format!("body {{ font-family: \"{quoted}\", system-ui, sans-serif !important; }}")
}

fn text(value: &Value) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.to_string()),
        Value::Value(inner) => text(inner),
        _ => None,
    }
}

fn read(proxy: &Proxy, (namespace, key): (&str, &str)) -> Option<String> {
    let found = proxy
        .call::<_, _, OwnedValue>("ReadOne", &(namespace, key))
        .or_else(|_| proxy.call::<_, _, OwnedValue>("Read", &(namespace, key)))
        .ok()?;
    text(&found)
}

fn from_portal() -> Option<String> {
    let connection = Connection::session().ok()?;
    let proxy = Proxy::new(&connection, DESTINATION, PATH, INTERFACE).ok()?;
    read(&proxy, GNOME)
        .as_deref()
        .and_then(from_gnome)
        .or_else(|| read(&proxy, KDE).as_deref().and_then(from_kde))
}

fn from_kde_file() -> Option<String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    from_kdeglobals(&std::fs::read_to_string(config.join("kdeglobals")).ok()?)
}

pub fn desktop() -> Option<String> {
    std::env::var("SOUNDSCAPE_FONT")
        .ok()
        .and_then(|family| clean(&family))
        .or_else(from_portal)
        .or_else(from_kde_file)
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
        let globals = "[Colors:View]\nfont=Wrong,9\n\n[General]\nColorScheme=Breeze\nfont=Inter Variable,10,-1,5,400,0,0,0,0,0\n";
        assert_eq!(from_kdeglobals(globals), Some("Inter Variable".into()));
        assert_eq!(from_kdeglobals("[General]\nColorScheme=Breeze\n"), None);
    }

    #[test]
    fn keeps_the_page_font_rule_safe() {
        assert_eq!(
            stylesheet("Adwaita Sans"),
            "body { font-family: \"Adwaita Sans\", system-ui, sans-serif !important; }"
        );
        assert_eq!(
            stylesheet("Odd \"Font\"\\\n"),
            "body { font-family: \"Odd \\\"Font\\\"\\\\\", system-ui, sans-serif !important; }"
        );
    }
}
