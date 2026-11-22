use crate::host::{AppHandle, Manager};
use crate::{settings, AppState, MAIN};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::Notify;
use url::Url;

const EVERY: Duration = Duration::from_millis(500);
const IDLE_EVERY: Duration = Duration::from_secs(10);
const MOST_AT_ONCE: usize = 20;
pub const PATH: &str = "/settings";
pub const SECTION: &str = "this-app";
static WAKE: Notify = Notify::const_new();

pub fn wake() {
    WAKE.notify_one();
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "do", rename_all = "kebab-case")]
pub enum Command {
    Option {
        name: String,
        value: bool,
    },
    Player {
        id: String,
        value: bool,
    },
    DiscordApp {
        id: String,
    },
    Pause,
    MemoryLimit {
        mb: u32,
    },
    KeepWindow {
        seconds: i64,
    },
    AddFix {
        artist: String,
        title: String,
        new_artist: String,
        new_title: String,
        every_song: bool,
    },
    RemoveFix {
        index: usize,
    },
    ChangeServer,
    SignOut,
    SendNow,
    TestNotification,
    Download,
    Window,
    Restart,
}

#[derive(Debug, Deserialize)]
struct Asked {
    page: String,
    commands: Vec<Value>,
}

pub const READ: &str = r#"(() => {
  if (location.pathname !== "/settings") return "null";
  window.soundscapeAppPage ??= String(Math.random()).slice(2);
  const queued = Array.isArray(window.soundscapeAppCommands) ? window.soundscapeAppCommands.splice(0, 20) : [];
  return JSON.stringify({ page: window.soundscapeAppPage, commands: queued });
})()"#;

pub fn commands(values: Vec<Value>) -> Vec<Command> {
    values
        .into_iter()
        .take(MOST_AT_ONCE)
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

pub fn push_script(payload: &Value) -> String {
    let text = serde_json::to_string(&payload.to_string()).unwrap_or_else(|_| "\"null\"".into());
    format!(
        r#"(() => {{ const detail = JSON.parse({text}); window.soundscapeAppSettings = detail; window.dispatchEvent(new CustomEvent("soundscape-app-settings", {{ detail }})); }})()"#
    )
}

pub fn is_settings_page(server: Option<&Url>, shown: &Url) -> bool {
    server.is_some_and(|server| server.origin() == shown.origin()) && shown.path() == PATH
}

pub fn page_address(server: &Url) -> Option<Url> {
    let mut page = server.join(PATH).ok()?;
    page.set_fragment(Some(SECTION));
    Some(page)
}

pub fn open(app: &AppHandle) {
    wake();
    let state = app.state::<AppState>();
    let config = state.config();
    let page = state
        .server()
        .filter(|_| config.connected())
        .and_then(|server| page_address(&server));
    match page {
        Some(page) => {
            if crate::open_main(app, page).is_err() {
                let _ = crate::open_settings(app);
            }
        }
        None => {
            let _ = crate::open_settings(app);
        }
    }
}

fn on_settings_page(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window(MAIN) else {
        return false;
    };
    let server = app.state::<AppState>().server();
    window
        .url()
        .is_ok_and(|shown| is_settings_page(server.as_ref(), &shown))
}

async fn perform(app: &AppHandle, command: Command) -> Result<(), String> {
    match command {
        Command::Option { name, value } => settings::set_option(app.clone(), name, value)
            .await
            .map(|_| ()),
        Command::Player { id, value } => {
            if settings::view(app)
                .players
                .iter()
                .any(|player| player.id == id)
            {
                settings::set_player(app.clone(), id, value);
            }
            Ok(())
        }
        Command::DiscordApp { id } => settings::set_discord_app(app.clone(), id).map(|_| ()),
        Command::Pause => {
            crate::toggle_pause(app);
            Ok(())
        }
        Command::MemoryLimit { mb } => {
            let mb = crate::allowed_memory_limit(mb);
            app.state::<AppState>()
                .update(|config| config.memory_limit = Some(mb));
            app.host().memory_limit(mb);
            Ok(())
        }
        Command::KeepWindow { seconds } => {
            let seconds = crate::allowed_keep_window(seconds);
            app.state::<AppState>()
                .update(|config| config.keep_window = Some(seconds));
            app.host().keep_window(seconds);
            Ok(())
        }
        Command::AddFix {
            artist,
            title,
            new_artist,
            new_title,
            every_song,
        } => settings::add_fix(
            app.clone(),
            artist,
            title,
            new_artist,
            new_title,
            every_song,
        )
        .map(|_| ()),
        Command::RemoveFix { index } => {
            settings::remove_fix(app.clone(), index);
            Ok(())
        }
        Command::ChangeServer => crate::open_setup(app).map_err(|error| error.to_string()),
        Command::SignOut => {
            crate::sign_out(app);
            Ok(())
        }
        Command::SendNow => settings::send_now(app.clone()).await.map(|_| ()),
        Command::TestNotification => settings::test_notification(app.clone()).await,
        Command::Download => {
            crate::updates::open_downloads(app);
            Ok(())
        }
        Command::Window => crate::open_settings(app).map_err(|error| error.to_string()),
        Command::Restart => {
            app.restart();
            Ok(())
        }
    }
}

pub async fn run(app: AppHandle) {
    let mut last: Option<(String, String)> = None;
    loop {
        if !on_settings_page(&app) {
            last = None;
            tokio::select! {
                _ = WAKE.notified() => {}
                _ = tokio::time::sleep(IDLE_EVERY) => {}
            }
            continue;
        }
        tokio::time::sleep(EVERY).await;
        if !on_settings_page(&app) {
            last = None;
            continue;
        }
        let Some(text) = crate::media::evaluate(&app, READ).await else {
            continue;
        };
        let Ok(asked) = serde_json::from_str::<Asked>(&text) else {
            continue;
        };
        let mut error = None;
        for command in commands(asked.commands) {
            if let Err(problem) = perform(&app, command).await {
                error = Some(problem);
            }
        }
        let payload = json!({ "settings": settings::view(&app), "error": error });
        let shown = (asked.page, payload.to_string());
        if error.is_none() && last.as_ref() == Some(&shown) {
            continue;
        }
        if crate::media::evaluate(&app, push_script(&payload))
            .await
            .is_some()
        {
            last = Some(shown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_only_the_commands_it_knows() {
        let asked = vec![
            json!({ "do": "option", "name": "scrobbling", "value": false }),
            json!({ "do": "player", "id": "spotify", "value": true }),
            json!({ "do": "discord-app", "id": "1234567890123456789" }),
            json!({ "do": "pause" }),
            json!({ "do": "send-now" }),
            json!({ "do": "test-notification" }),
            json!({ "do": "download" }),
            json!({ "do": "window" }),
            json!({ "do": "restart" }),
            json!({ "do": "sign-out" }),
            json!({ "do": "change-server" }),
            json!({ "do": "remove-fix", "index": 2 }),
            json!({ "do": "memory-limit", "mb": 700 }),
            json!({ "do": "add-fix", "artist": "a", "title": "t", "new_artist": "A", "new_title": "T", "every_song": false }),
            json!({ "do": "add-fix", "artist": "a" }),
            json!({ "do": "option", "name": "scrobbling", "value": "no" }),
            json!("pause"),
        ];
        assert_eq!(
            commands(asked),
            vec![
                Command::Option {
                    name: "scrobbling".into(),
                    value: false
                },
                Command::Player {
                    id: "spotify".into(),
                    value: true
                },
                Command::DiscordApp {
                    id: "1234567890123456789".into()
                },
                Command::Pause,
                Command::SendNow,
                Command::TestNotification,
                Command::Download,
                Command::Window,
                Command::Restart,
                Command::SignOut,
                Command::ChangeServer,
                Command::RemoveFix { index: 2 },
                Command::MemoryLimit { mb: 700 },
                Command::AddFix {
                    artist: "a".into(),
                    title: "t".into(),
                    new_artist: "A".into(),
                    new_title: "T".into(),
                    every_song: false
                },
            ]
        );
        assert_eq!(commands(vec![json!({ "do": "pause" }); 50]).len(), 20);
    }

    #[test]
    fn hands_the_page_its_settings_safely() {
        let script =
            push_script(&json!({ "settings": { "user": "</script>\"'`" }, "error": null }));
        assert!(script.starts_with("(() => { const detail = JSON.parse(\""));
        assert!(script.contains("soundscape-app-settings"));
        assert!(!script.contains("\"user\":\"</script>\"'`\""));
    }

    #[test]
    fn knows_when_the_settings_page_is_open() {
        let server = Url::parse("https://music.example").unwrap();
        let here = |text: &str| Url::parse(text).unwrap();
        assert!(is_settings_page(
            Some(&server),
            &here("https://music.example/settings")
        ));
        assert!(is_settings_page(
            Some(&server),
            &here("https://music.example/settings#this-app")
        ));
        assert!(!is_settings_page(
            Some(&server),
            &here("https://music.example/stats")
        ));
        assert!(!is_settings_page(
            Some(&server),
            &here("https://other.example/settings")
        ));
        assert!(!is_settings_page(
            None,
            &here("https://music.example/settings")
        ));
        assert_eq!(
            page_address(&server).unwrap().as_str(),
            "https://music.example/settings#this-app"
        );
    }
}
