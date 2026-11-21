use crate::host::AppHandle;
use std::collections::HashMap;
use url::Url;
use zbus::zvariant::Value;

pub const SAME_WINDOW: &str = concat!(
    r#"(() => {
  if (window.top !== window) return;
  Object.defineProperty(window, "soundscapeDesktop", { value: true });
  Object.defineProperty(window, "soundscapeDesktopVersion", { value: ""#,
    env!("CARGO_PKG_VERSION"),
    r#"" });
  document.addEventListener(
    "click",
    (event) => {
      const link = event.target instanceof Element ? event.target.closest("a[href][target]") : null;
      if (link && link.target !== "_self") link.target = "_self";
    },
    true,
  );
  window.open = (address) => {
    if (address) window.location.assign(new URL(String(address), window.location.href).href);
    return null;
  };
})();"#
);

pub fn openable(target: &Url) -> bool {
    matches!(target.scheme(), "http" | "https" | "mailto")
}

async fn through_portal(target: &str) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    connection
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.OpenURI"),
            "OpenURI",
            &("", target, options),
        )
        .await?;
    Ok(())
}

pub fn open(_app: &AppHandle, target: &Url) {
    if !openable(target) {
        return;
    }
    let target = target.to_string();
    crate::host::async_runtime::spawn(async move {
        if through_portal(&target).await.is_ok() {
            return;
        }
        if let Err(error) = crate::host::open_url(target.as_str()) {
            eprintln!("Couldn't open {target}: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_the_page_which_version_it_runs_in() {
        let line = format!(
            r#"Object.defineProperty(window, "soundscapeDesktopVersion", {{ value: "{}" }});"#,
            env!("CARGO_PKG_VERSION")
        );
        assert!(SAME_WINDOW.contains(&line));
    }

    #[test]
    fn opens_only_web_and_mail_links_outside() {
        assert!(openable(
            &Url::parse("https://artist.bandcamp.com/").unwrap()
        ));
        assert!(openable(&Url::parse("http://example.com/").unwrap()));
        assert!(openable(&Url::parse("mailto:hi@example.com").unwrap()));
        assert!(!openable(&Url::parse("file:///etc/passwd").unwrap()));
        assert!(!openable(&Url::parse("javascript:alert(1)").unwrap()));
    }
}
