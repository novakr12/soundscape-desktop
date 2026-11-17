use std::collections::HashMap;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
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

pub const FRAME_COUNTER: &str = r#"(() => {
  if (window.top !== window) return;
  const show = () => {
    const box = document.createElement("div");
    box.setAttribute("aria-hidden", "true");
    box.style.cssText = "position:fixed;top:6px;right:6px;z-index:2147483647;padding:2px 6px;border-radius:4px;background:rgba(0,0,0,.75);color:#00ff66;font:12px monospace;pointer-events:none";
    box.textContent = "… fps";
    document.documentElement.append(box);
    let frames = 0;
    let since = performance.now();
    const tick = (now) => {
      frames += 1;
      if (now - since >= 500) {
        box.textContent = `${Math.round((frames * 1000) / (now - since))} fps`;
        frames = 0;
        since = now;
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  };
  if (document.documentElement) show();
  else document.addEventListener("DOMContentLoaded", show);
})();"#;

pub fn wants_frame_counter(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes"))
}

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

pub fn open(app: &AppHandle, target: &Url) {
    if !openable(target) {
        return;
    }
    let app = app.clone();
    let target = target.to_string();
    tauri::async_runtime::spawn(async move {
        if through_portal(&target).await.is_ok() {
            return;
        }
        if let Err(error) = app.opener().open_url(&target, None::<&str>) {
            eprintln!("Couldn't open {target}: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_frame_counter_only_when_asked() {
        assert!(wants_frame_counter(Some("1")));
        assert!(wants_frame_counter(Some("true")));
        assert!(!wants_frame_counter(Some("0")));
        assert!(!wants_frame_counter(Some("")));
        assert!(!wants_frame_counter(None));
        assert!(FRAME_COUNTER.contains("requestAnimationFrame"));
    }

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
