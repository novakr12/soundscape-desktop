pub mod autostart;
pub mod config;
pub mod discord;
pub mod fixes;
pub mod friends;
pub mod host;
pub mod instance;
pub mod links;
pub mod love;
pub mod media;
pub mod mini;
pub mod mpris;
pub mod music;
pub mod native;
pub mod native_link;
pub mod notify;
pub mod page;
pub mod queue;
pub mod recap;
pub mod relay;
pub mod rendering;
pub mod scrobble;
pub mod scrobbler;
pub mod search;
pub mod server;
pub mod settings;
pub mod settings_page;
pub mod setup;
pub mod state;
pub mod submit;
pub mod tidy;
pub mod today;
pub mod tray;
pub mod updates;
pub mod yearscape;

use config::Config;
use host::{async_runtime, AppHandle, Host, Manager, Running};
use std::path::PathBuf;
use url::Url;

pub use state::AppState;

pub const MAIN: &str = "main";
pub const SETUP: &str = "setup";
pub const IDENTIFIER: &str = "io.github.novakr12.SoundscapeDesktop";
pub const MEMORY_LIMIT_MB: u32 = 500;
pub const MEMORY_LIMITS_MB: [u32; 4] = [0, 400, 500, 700];

pub fn allowed_memory_limit(limit_mb: u32) -> u32 {
    if MEMORY_LIMITS_MB.contains(&limit_mb) {
        limit_mb
    } else {
        MEMORY_LIMIT_MB
    }
}

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

pub fn launch(host: Box<dyn Host>, dir: PathBuf, running: Running) -> AppHandle {
    let app = AppHandle::new(host);
    rendering::remember(running);
    app.manage(AppState::new(dir.clone(), Config::load(&dir)));
    async_runtime::spawn(scrobbler::run(app.clone()));
    async_runtime::spawn(notify::run(app.clone()));
    async_runtime::spawn(friends::run(app.clone()));
    async_runtime::spawn(music::run(app.clone()));
    async_runtime::spawn(love::run(app.clone()));
    async_runtime::spawn(today::run(app.clone()));
    async_runtime::spawn(yearscape::run(app.clone()));
    async_runtime::spawn(updates::run(app.clone()));
    async_runtime::spawn(settings_page::run(app.clone()));
    discord::run(app.clone());
    async_runtime::spawn(recap::run(app.clone()));
    async_runtime::spawn(media::run(app.clone()));
    async_runtime::spawn(search::run(app.clone()));
    async_runtime::spawn(mini::run(app.clone()));
    async_runtime::spawn(native_link::run(app.clone()));
    async_runtime::spawn(native_link::link(app.clone()));
    tray::create(&app);
    app
}

pub fn first_page(app: &AppHandle) -> Option<Url> {
    let state = app.state::<AppState>();
    let config = state.config();
    let server = state.server()?;
    Some(if config.connected() {
        server
    } else {
        server::connect_url(&server, &server::device_name())
    })
}

pub fn start_page(app: &AppHandle) -> Url {
    first_page(app).unwrap_or_else(|| setup::page("", None))
}

pub fn follow_from_window(app: &AppHandle, target: &Url) {
    if setup::follow(app, target) {
        return;
    }
    let server = app.state::<AppState>().server();
    match server::classify(server.as_ref(), target) {
        server::Navigation::AppSettings => {
            let _ = open_settings(app);
        }
        server::Navigation::External => links::open(app, target),
        server::Navigation::Connected { .. } => {
            connected_through(app, target);
        }
        server::Navigation::Allow | server::Navigation::Block => {}
    }
}

pub fn open_main(app: &AppHandle, start: Url) -> host::Result<()> {
    let host = app.host();
    host.navigate(MAIN, start);
    host.show(MAIN);
    host.focus(MAIN);
    tray::refresh(app);
    Ok(())
}

pub fn open_settings(app: &AppHandle) -> host::Result<()> {
    match app.state::<AppState>().server() {
        Some(server) => match settings_page::page_address(&server) {
            Some(page) => open_main(app, page),
            None => open_main(app, server),
        },
        None => open_setup(app),
    }
}

pub fn open_setup(app: &AppHandle) -> host::Result<()> {
    app.host()
        .open_window(SETUP, "index.html", &host::WindowOptions::default())
}

pub fn show(app: &AppHandle) {
    let host = app.host();
    host.show(MAIN);
    host.focus(MAIN);
}

pub fn toggle_pause(app: &AppHandle) {
    let state = app.state::<AppState>();
    let now = config::now_seconds();
    state.update(|config| {
        config.paused_until = match config.paused_minutes(now) {
            Some(_) => None,
            None => Some(now + config::PAUSE_SECONDS),
        };
    });
    tray::refresh(app);
}

pub fn connect_app(app: &AppHandle) {
    let state = app.state::<AppState>();
    match state.server() {
        Some(server) => {
            let _ = open_main(app, server::connect_url(&server, &server::device_name()));
        }
        None => {
            let _ = open_setup(app);
        }
    }
}

pub fn sign_out(app: &AppHandle) {
    let state = app.state::<AppState>();
    let previous = state.config();
    state.update(|config| {
        config.token = None;
        config.user = None;
        config.last_notification = None;
    });
    state.set_signed_out(false);
    let _ = open_setup(app);
    async_runtime::spawn(async move {
        if let (Some(server), Some(token)) = (
            previous.server.as_deref().and_then(|s| Url::parse(s).ok()),
            previous.token,
        ) {
            server::revoke_token(&server, &token).await;
        }
    });
}

pub fn stays_running(config: &Config) -> bool {
    config.connected() && config.keep_running
}

pub fn window_closed(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let config = state.config();
    if !stays_running(&config) {
        return false;
    }
    if !config.told_about_background {
        state.update(|config| config.told_about_background = true);
        let app = app.clone();
        async_runtime::spawn(async move {
            let _ = notify::show(
                &app,
                "Soundscape is still running",
                "It keeps sending what you play. Open it again from your apps, or quit from the tray.",
                notify::Click::Window,
            )
            .await;
        });
    }
    true
}

pub fn starts_hidden(args: &[String], config: &Config) -> bool {
    args.iter().any(|arg| arg == autostart::BACKGROUND_FLAG) && config.connected()
}

pub fn connected_through(app: &AppHandle, target: &Url) -> bool {
    let state = app.state::<AppState>();
    let server::Navigation::Connected { token, user } =
        server::classify(state.server().as_ref(), target)
    else {
        return false;
    };
    let config = state.update(|config| {
        config.token = Some(token);
        config.user = Some(user);
        config.last_notification = None;
    });
    state.set_signed_out(false);
    let app = app.clone();
    async_runtime::spawn(async move {
        notify::check(&app).await;
        if let Some(home) = config.server.and_then(|s| Url::parse(&s).ok()) {
            app.host().navigate(MAIN, home);
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_running_after_the_window_closes_only_when_connected_and_asked_to() {
        let connected = Config {
            server: Some("https://music.example.com/".into()),
            token: Some("t".into()),
            ..Config::default()
        };
        assert!(stays_running(&connected));
        assert!(!stays_running(&Config {
            keep_running: false,
            ..connected.clone()
        }));
        assert!(!stays_running(&Config::default()));
    }

    #[test]
    fn starts_hidden_at_login_only_once_connected() {
        let connected = Config {
            server: Some("https://music.example.com/".into()),
            token: Some("t".into()),
            ..Config::default()
        };
        let login = vec!["soundscape-desktop".to_string(), "--background".to_string()];
        assert!(starts_hidden(&login, &connected));
        assert!(!starts_hidden(&login, &Config::default()));
        assert!(!starts_hidden(&login[..1], &connected));
    }

    #[test]
    fn keeps_its_settings_where_the_old_app_did() {
        assert_eq!(
            config_dir_from(None, Some(PathBuf::from("/home/u"))),
            Some(PathBuf::from(
                "/home/u/.config/io.github.novakr12.SoundscapeDesktop"
            ))
        );
        assert_eq!(
            config_dir_from(
                Some(PathBuf::from("/x/config")),
                Some(PathBuf::from("/home/u"))
            ),
            Some(PathBuf::from(
                "/x/config/io.github.novakr12.SoundscapeDesktop"
            ))
        );
        assert_eq!(allowed_memory_limit(700), 700);
        assert_eq!(allowed_memory_limit(123), MEMORY_LIMIT_MB);
    }
}
