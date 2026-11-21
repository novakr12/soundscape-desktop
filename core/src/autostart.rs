use std::collections::HashMap;
use std::fs;
use std::future::poll_fn;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;
use zbus::export::futures_core::Stream;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

pub const APP_ID: &str = "io.github.novakr12.SoundscapeDesktop";
pub const BACKGROUND_FLAG: &str = "--background";
const PORTAL_WAIT: Duration = Duration::from_secs(120);

pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

fn quote(path: &str) -> String {
    let mut quoted = String::from("\"");
    for character in path.chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

pub fn desktop_entry(executable: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Soundscape\nComment=Send what you play to Soundscape\nExec={} {BACKGROUND_FLAG}\nIcon={APP_ID}\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
        quote(executable)
    )
}

pub fn entry_path(config_home: &Path) -> PathBuf {
    config_home
        .join("autostart")
        .join(format!("{APP_ID}.desktop"))
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

pub fn set_file(config_home: &Path, executable: &str, on: bool) -> std::io::Result<()> {
    let path = entry_path(config_home);
    if on {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, desktop_entry(executable))
    } else {
        match fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

pub fn request_path(unique_name: &str, token: &str) -> String {
    let sender = unique_name.trim_start_matches(':').replace('.', "_");
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

async fn next<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    poll_fn(|context| Pin::new(&mut *stream).poll_next(context)).await
}

async fn ask_portal(on: bool) -> Result<bool, String> {
    static REQUEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let connection = zbus::Connection::session()
        .await
        .map_err(|error| error.to_string())?;
    let seq = REQUEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let token = format!("soundscape_{}_{seq}", std::process::id());
    let unique = connection
        .unique_name()
        .ok_or_else(|| "Couldn't read the D-Bus connection name.".to_string())?;
    let expected = OwnedObjectPath::try_from(request_path(unique.as_str(), &token))
        .map_err(|error| error.to_string())?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        expected.clone(),
        "org.freedesktop.portal.Request",
    )
    .await
    .map_err(|error| error.to_string())?;
    let mut fallback_proxy = None;
    let mut responses = proxy
        .receive_signal("Response")
        .await
        .map_err(|error| error.to_string())?;

    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert(
        "reason",
        Value::from("Keep sending what you play to Soundscape after you log in."),
    );
    options.insert("autostart", Value::from(on));
    options.insert(
        "commandline",
        Value::from(vec!["soundscape-desktop", BACKGROUND_FLAG]),
    );
    let reply = connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.Background"),
            "RequestBackground",
            &("", options),
        )
        .await
        .map_err(|_| "This desktop doesn't offer a way to start apps at login.".to_string())?;
    let actual: OwnedObjectPath = reply
        .body()
        .deserialize()
        .map_err(|error| error.to_string())?;
    if actual != expected {
        let fallback = fallback_proxy.insert(
            zbus::Proxy::new(
                &connection,
                "org.freedesktop.portal.Desktop",
                actual,
                "org.freedesktop.portal.Request",
            )
            .await
            .map_err(|error| error.to_string())?,
        );
        responses = fallback
            .receive_signal("Response")
            .await
            .map_err(|error| error.to_string())?;
    }
    match tokio::time::timeout(PORTAL_WAIT, next(&mut responses)).await {
        Ok(Some(message)) => {
            let (code, results): (u32, HashMap<String, OwnedValue>) = message
                .body()
                .deserialize()
                .map_err(|error| error.to_string())?;
            let granted = results
                .get("autostart")
                .and_then(|value| bool::try_from(value.clone()).ok())
                .unwrap_or(false);
            Ok(code == 0 && (granted || !on))
        }
        Ok(None) => Err("Your desktop closed the login permission request.".into()),
        Err(_) => Err("Your desktop didn't answer the login permission request in time.".into()),
    }
}

pub async fn set(on: bool) -> Result<bool, String> {
    if in_flatpak() {
        return ask_portal(on).await;
    }
    let home = config_home().ok_or("Couldn't find your settings folder.")?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    set_file(&home, &executable.to_string_lossy(), on).map_err(|error| error.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicts_portal_request_path_from_unique_name() {
        assert_eq!(
            request_path(":1.42", "soundscape_7_0"),
            "/org/freedesktop/portal/desktop/request/1_42/soundscape_7_0"
        );
    }

    #[test]
    fn writes_a_login_entry_that_starts_hidden() {
        let entry = desktop_entry("/home/me/My Apps/soundscape-desktop");
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/home/me/My Apps/soundscape-desktop\" --background\n"));
        assert!(entry.contains("\nIcon=io.github.novakr12.SoundscapeDesktop\n"));
        assert!(desktop_entry("/odd/$name\"x").contains(r#"Exec="/odd/\$name\"x" --background"#));
    }

    #[test]
    fn adds_and_removes_the_entry() {
        let home = std::env::temp_dir().join(format!(
            "soundscape-desktop-autostart-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&home);
        set_file(&home, "/usr/bin/soundscape-desktop", true).unwrap();
        assert!(fs::read_to_string(entry_path(&home))
            .unwrap()
            .contains("Exec=\"/usr/bin/soundscape-desktop\" --background"));
        set_file(&home, "/usr/bin/soundscape-desktop", false).unwrap();
        assert!(!entry_path(&home).exists());
        set_file(&home, "/usr/bin/soundscape-desktop", false).unwrap();
        let _ = fs::remove_dir_all(&home);
    }
}
