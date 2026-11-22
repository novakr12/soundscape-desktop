use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use url::Url;

pub const WINDOW_NAME: &str = "soundscape-window";
pub const IDENTIFIER: &str = "io.github.novakr12.SoundscapeDesktop";
pub const APP_SCHEME: &str = "soundscape-desktop";
pub const CONNECTED_PATH: &str = "/desktop/connected";
pub const SETUP_HOST: &str = "setup.soundscape-desktop.invalid";
pub const NATIVE_ASKED: &str = "soundscape-native ";

pub const KEEP_WINDOW_SECONDS: i64 = 60;
pub const KEEP_WINDOW_ALWAYS: i64 = -1;
pub const KEEP_WINDOW_CHOICES: [i64; 5] = [0, 60, 300, 900, KEEP_WINDOW_ALWAYS];

pub fn allowed_keep_window(seconds: i64) -> i64 {
    if KEEP_WINDOW_CHOICES.contains(&seconds) {
        seconds
    } else {
        KEEP_WINDOW_SECONDS
    }
}

pub const SAME_WINDOW: &str = concat!(
    r#"(() => {
  if (window.top !== window) return;
  Object.defineProperty(window, "soundscapeDesktop", { value: true });
  Object.defineProperty(window, "soundscapeDesktopVersion", { value: ""#,
    env!("CARGO_PKG_VERSION"),
    r#"" });
  window.dispatchEvent(new Event("soundscape-desktop-ready"));
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

pub fn config_dir_from(config_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    config_home
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))
        .map(|dir| dir.join(IDENTIFIER))
}

pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

pub fn wants(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes"))
}

pub fn server_from(given: Option<&str>, saved: Option<&str>) -> Option<Url> {
    given
        .or(saved)
        .and_then(|text| Url::parse(text.trim()).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
}

pub fn given_server_arg() -> Option<String> {
    std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with("--"))
        .or_else(|| std::env::var("SOUNDSCAPE_SERVER").ok())
}

pub fn given_server() -> Option<Url> {
    server_from(given_server_arg().as_deref(), None)
}

pub mod server {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    pub enum Navigation {
        Allow,
        AppSettings,
        Connected { token: String, user: String },
        External,
        Block,
    }

    fn connected(pairs: url::form_urlencoded::Parse<'_>) -> Option<Navigation> {
        let pairs: HashMap<_, _> = pairs.into_owned().collect();
        match (pairs.get("token"), pairs.get("user")) {
            (Some(token), Some(user)) if !token.is_empty() && !user.is_empty() => {
                Some(Navigation::Connected {
                    token: token.clone(),
                    user: user.clone(),
                })
            }
            _ => None,
        }
    }

    pub fn classify(server: Option<&Url>, target: &Url) -> Navigation {
        if target.scheme() == APP_SCHEME {
            if target.host_str() == Some("settings") {
                return Navigation::AppSettings;
            }
            if target.host_str() != Some("connected") {
                return Navigation::Block;
            }
            return connected(target.query_pairs()).unwrap_or(Navigation::Block);
        }
        if server.is_some_and(|server| server.origin() == target.origin())
            && target.path() == CONNECTED_PATH
            && let Some(found) = target
                .fragment()
                .and_then(|fragment| connected(url::form_urlencoded::parse(fragment.as_bytes())))
        {
            return found;
        }
        match target.scheme() {
            "about" | "data" | "blob" => Navigation::Allow,
            "http" | "https" => {
                if server.is_some_and(|server| server.origin() == target.origin()) {
                    Navigation::Allow
                } else {
                    Navigation::External
                }
            }
            "mailto" => Navigation::External,
            _ => Navigation::Block,
        }
    }
}

pub mod setup {
    use super::*;

    const STYLE: &str = "body{margin:0;min-height:100vh;display:grid;place-items:center;font:16px system-ui,sans-serif;color-scheme:light dark;background:Canvas;color:CanvasText}main{width:min(420px,calc(100vw - 48px))}h1{font-size:1.5rem;margin:0 0 .5rem}p{line-height:1.5}.muted{opacity:.7}label{display:block;font-weight:600;margin:1.25rem 0 .4rem}input{box-sizing:border-box;width:100%;font:inherit;padding:.6rem .75rem;border:1px solid color-mix(in srgb,CanvasText 30%,transparent);border-radius:8px;background:Canvas;color:CanvasText}button{margin-top:1rem;font:inherit;font-weight:600;padding:.6rem 1.2rem;border:0;border-radius:8px;background:#1db954;color:#fff;cursor:pointer}.error{color:#d93025;font-weight:600}";

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    pub fn page(address: &str, problem: Option<&str>) -> Url {
        let problem = problem
            .map(|text| format!("<p class=\"error\" role=\"alert\">{}</p>", escape(text)))
            .unwrap_or_default();
        let html = format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Connect to Soundscape</title><style>{STYLE}</style><main><h1>Connect to Soundscape</h1><p class=\"muted\">Enter the address you open Soundscape at in your browser.</p><form action=\"http://{SETUP_HOST}/\" method=\"get\"><label for=\"server\">Server address</label><input id=\"server\" name=\"server\" type=\"text\" inputmode=\"url\" autocomplete=\"url\" spellcheck=\"false\" placeholder=\"https://music.example.com\" value=\"{value}\" required autofocus><button type=\"submit\">Connect</button>{problem}</form><p class=\"muted\">You'll log in on the next page. The app gets its own token, which you can revoke on Soundscape's Connect page.</p></main></html>",
            value = escape(address),
        );
        let encoded: String = url::form_urlencoded::byte_serialize(html.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        Url::parse(&format!("data:text/html;charset=utf-8,{encoded}")).expect("setup page")
    }

    pub fn asked(target: &Url) -> Option<String> {
        let ours =
            matches!(target.scheme(), "http" | "https") && target.host_str() == Some(SETUP_HOST);
        if !ours {
            return None;
        }
        Some(
            target
                .query_pairs()
                .find(|(name, _)| name == "server")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default(),
        )
    }
}

pub mod theme {
    pub fn from_scheme_dark(scheme: u32) -> bool {
        scheme == 1
    }

    pub fn chosen_dark(value: Option<&str>) -> Option<bool> {
        match value.map(str::trim) {
            Some("dark") => Some(true),
            Some("light") => Some(false),
            _ => None,
        }
    }

    pub fn backdrop_rgba(dark: bool) -> [f64; 4] {
        if dark {
            [17.0 / 255.0, 17.0 / 255.0, 17.0 / 255.0, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Start {
    pub url: String,
    pub server: Option<String>,
    pub native: Option<String>,
    pub size: Option<(u32, u32)>,
    pub maximized: bool,
    pub smooth: bool,
    pub memory_limit: u32,
    #[serde(default = "standard_keep_window")]
    pub keep_window: i64,
    #[serde(default)]
    pub cpu_rendering: bool,
    #[serde(default)]
    pub faster_scripts: bool,
    #[serde(default)]
    pub dark: bool,
    #[serde(default)]
    pub font: Option<String>,
}

fn standard_keep_window() -> i64 {
    KEEP_WINDOW_SECONDS
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum ToWindow {
    Start(Start),
    Eval { script: String },
    Evaluate { id: u64, script: String },
    Show,
    Hide,
    Focus,
    Navigate { url: String },
    Server { url: Option<String> },
    Native { script: Option<String> },
    Setup,
    Smooth { on: bool },
    Memory { mb: u32 },
    KeepWindow { seconds: i64 },
    Theme { dark: bool },
    Font { family: Option<String> },
    Exit,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum FromWindow {
    Answer {
        id: u64,
        text: Option<String>,
    },
    Url {
        url: String,
    },
    Hidden {
        hidden: bool,
    },
    Page {
        message: String,
    },
    Follow {
        url: String,
    },
    Close,
    Shortcut {
        name: String,
    },
    Poke,
    WindowState {
        width: u32,
        height: u32,
        maximized: bool,
    },
    CpuRendering {
        on: bool,
    },
    Done,
}

pub fn send<T: Serialize>(out: &mut impl Write, message: &T) -> bool {
    let Ok(mut line) = serde_json::to_string(message) else {
        return false;
    };
    line.push('\n');
    out.write_all(line.as_bytes())
        .and_then(|_| out.flush())
        .is_ok()
}

pub fn read<T: for<'a> Deserialize<'a>>(input: &mut impl BufRead) -> Option<Option<T>> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(serde_json::from_str(line.trim_end()).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_messages_across_a_line_pipe() {
        let mut wire = Vec::new();
        assert!(send(
            &mut wire,
            &ToWindow::Evaluate {
                id: 7,
                script: "1 + 1\n".into()
            }
        ));
        assert!(send(&mut wire, &ToWindow::Show));
        assert!(send(
            &mut wire,
            &FromWindow::Answer {
                id: 7,
                text: Some("2".into())
            }
        ));
        let mut input = std::io::Cursor::new(wire);
        assert_eq!(
            read::<ToWindow>(&mut input),
            Some(Some(ToWindow::Evaluate {
                id: 7,
                script: "1 + 1\n".into()
            }))
        );
        assert_eq!(read::<ToWindow>(&mut input), Some(Some(ToWindow::Show)));
        assert_eq!(
            read::<FromWindow>(&mut input),
            Some(Some(FromWindow::Answer {
                id: 7,
                text: Some("2".into())
            }))
        );
        assert_eq!(read::<FromWindow>(&mut input), None);
    }

    #[test]
    fn keeps_reading_after_an_unreadable_line() {
        let mut input = std::io::Cursor::new(b"{bad}\n{\"do\":\"poke\"}\n");
        assert_eq!(read::<FromWindow>(&mut input), Some(None));
        assert_eq!(read::<FromWindow>(&mut input), Some(Some(FromWindow::Poke)));
    }

    #[test]
    fn restores_the_window_start_record() {
        let start = Start {
            url: "https://music.example.com/".into(),
            server: Some("https://music.example.com/".into()),
            native: None,
            size: Some((900, 700)),
            maximized: false,
            smooth: true,
            memory_limit: 500,
            keep_window: 300,
            cpu_rendering: false,
            faster_scripts: false,
            dark: true,
            font: Some("Inter".into()),
        };
        let mut wire = Vec::new();
        send(&mut wire, &ToWindow::Start(start.clone()));
        let mut input = std::io::Cursor::new(wire);
        assert_eq!(
            read::<ToWindow>(&mut input),
            Some(Some(ToWindow::Start(start)))
        );
    }

    #[test]
    fn carries_theme_and_font_updates() {
        let mut wire = Vec::new();
        assert!(send(&mut wire, &ToWindow::Theme { dark: true }));
        assert!(send(
            &mut wire,
            &ToWindow::Font {
                family: Some("Adwaita Sans".into())
            }
        ));
        let mut input = std::io::Cursor::new(wire);
        assert_eq!(
            read::<ToWindow>(&mut input),
            Some(Some(ToWindow::Theme { dark: true }))
        );
        assert_eq!(
            read::<ToWindow>(&mut input),
            Some(Some(ToWindow::Font {
                family: Some("Adwaita Sans".into())
            }))
        );
    }
}
