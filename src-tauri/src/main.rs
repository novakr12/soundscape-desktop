mod autostart;
mod config;
mod discord;
mod fixes;
mod friends;
mod handover;
mod links;
mod love;
mod media;
mod menu;
mod mini;
mod mpris;
mod music;
mod native;
mod notify;
mod queue;
mod recap;
mod relay;
mod rendering;
mod scrobble;
mod scrobbler;
mod search;
mod server;
mod settings;
mod settings_page;
mod submit;
mod tidy;
mod today;
mod tray;
mod updates;
mod yearscape;

use config::Config;
use server::Navigation;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{
    AppHandle, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent,
};
use url::Url;

pub const MAIN: &str = "main";
const SETUP: &str = "setup";
const SETTINGS: &str = "settings";

pub struct AppState {
    dir: PathBuf,
    config: Mutex<Config>,
    signed_out: AtomicBool,
    status: Mutex<scrobbler::Status>,
    friends: Mutex<Vec<friends::Friend>>,
    playlists: Mutex<Vec<music::Playlist>>,
    loved: Mutex<Option<(scrobble::Track, bool)>>,
    skip_request: Mutex<Option<scrobble::Track>>,
    skipped: Mutex<Option<scrobble::Track>>,
    today: Mutex<Option<today::Today>>,
    mini: Mutex<Option<mini::Source>>,
    preview: Mutex<Option<media::Preview>>,
    update: Mutex<Option<String>>,
    native: Mutex<Option<native::Player>>,
}

impl AppState {
    pub fn config(&self) -> Config {
        self.config.lock().expect("config lock").clone()
    }

    pub fn update(&self, change: impl FnOnce(&mut Config)) -> Config {
        let mut config = self.config.lock().expect("config lock");
        change(&mut config);
        if let Err(error) = config.save(&self.dir) {
            eprintln!("Couldn't save settings: {error}");
        }
        config.clone()
    }

    pub fn server(&self) -> Option<Url> {
        self.config()
            .server
            .and_then(|server| Url::parse(&server).ok())
    }

    pub fn signed_out(&self) -> bool {
        self.signed_out.load(Ordering::Relaxed)
    }

    pub fn set_signed_out(&self, value: bool) {
        self.signed_out.store(value, Ordering::Relaxed);
    }

    pub fn status(&self) -> scrobbler::Status {
        self.status.lock().expect("status lock").clone()
    }

    pub fn friends(&self) -> Vec<friends::Friend> {
        self.friends.lock().expect("friends lock").clone()
    }

    pub fn set_friends(&self, friends: Vec<friends::Friend>) -> bool {
        let mut current = self.friends.lock().expect("friends lock");
        if *current == friends {
            return false;
        }
        *current = friends;
        true
    }

    pub fn playlists(&self) -> Vec<music::Playlist> {
        self.playlists.lock().expect("playlists lock").clone()
    }

    pub fn set_playlists(&self, playlists: Vec<music::Playlist>) -> bool {
        let mut current = self.playlists.lock().expect("playlists lock");
        if *current == playlists {
            return false;
        }
        *current = playlists;
        true
    }

    pub fn loved_for(&self, track: &scrobble::Track) -> Option<bool> {
        match &*self.loved.lock().expect("loved lock") {
            Some((known, loved)) if known.same_song(track) => Some(*loved),
            _ => None,
        }
    }

    pub fn set_loved(&self, track: &scrobble::Track, loved: bool) {
        *self.loved.lock().expect("loved lock") = Some((track.clone(), loved));
    }

    pub fn request_skip(&self, track: scrobble::Track) {
        *self.skip_request.lock().expect("skip lock") = Some(track);
    }

    pub fn take_skip_request(&self) -> Option<scrobble::Track> {
        self.skip_request.lock().expect("skip lock").take()
    }

    pub fn set_skipped(&self, track: Option<scrobble::Track>) {
        *self.skipped.lock().expect("skip lock") = track;
    }

    pub fn was_skipped(&self, track: &scrobble::Track) -> bool {
        self.skipped
            .lock()
            .expect("skip lock")
            .as_ref()
            .is_some_and(|skipped| skipped.same_song(track))
    }

    pub fn mini(&self) -> Option<mini::Source> {
        self.mini.lock().expect("mini lock").clone()
    }

    pub fn set_mini(&self, source: Option<mini::Source>) {
        *self.mini.lock().expect("mini lock") = source;
    }

    pub fn preview(&self) -> Option<media::Preview> {
        self.preview.lock().expect("preview lock").clone()
    }

    pub fn set_preview(&self, preview: Option<media::Preview>) {
        *self.preview.lock().expect("preview lock") = preview;
    }

    pub fn update_available(&self) -> Option<String> {
        self.update.lock().expect("update lock").clone()
    }

    pub fn set_update(&self, version: Option<String>) -> bool {
        let mut current = self.update.lock().expect("update lock");
        if *current == version {
            return false;
        }
        *current = version;
        true
    }

    pub fn today(&self) -> Option<today::Today> {
        *self.today.lock().expect("today lock")
    }

    pub fn set_today(&self, today: Option<today::Today>) -> bool {
        let mut current = self.today.lock().expect("today lock");
        if *current == today {
            return false;
        }
        *current = today;
        true
    }

    pub fn set_status(&self, status: scrobbler::Status) -> bool {
        let mut current = self.status.lock().expect("status lock");
        if *current == status {
            return false;
        }
        *current = status;
        true
    }
}

#[tauri::command]
fn current_server(state: State<'_, AppState>) -> Option<String> {
    state.config().server
}

#[tauri::command]
async fn connect_server(app: AppHandle, address: String) -> Result<(), String> {
    let server = server::normalize_address(&address)?;
    server::check_server(&server).await?;
    let state = app.state::<AppState>();
    let previous = state.config();
    if let (Some(old), Some(token)) = (
        previous.server.as_deref().and_then(|s| Url::parse(s).ok()),
        previous.token.as_deref(),
    ) {
        if old != server {
            server::revoke_token(&old, token).await;
        }
    }
    let same = previous.server.as_deref() == Some(server.as_str())
        && previous.connected()
        && !state.signed_out();
    if !same {
        state.update(|config| {
            config.server = Some(server.to_string());
            config.user = None;
            config.token = None;
            config.last_notification = None;
        });
    }
    let start = if same {
        server.clone()
    } else {
        server::connect_url(&server, &server::device_name())
    };
    open_main(&app, start).map_err(|error| error.to_string())?;
    if let Some(setup) = app.get_webview_window(SETUP) {
        let _ = setup.close();
    }
    Ok(())
}

pub fn open_settings(app: &AppHandle) -> tauri::Result<()> {
    open_settings_at(app, None)
}

pub fn open_settings_at(app: &AppHandle, part: Option<&str>) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        window.show()?;
        if let Some(part) = part {
            let _ = window.eval(format!(
                "document.getElementById({part:?})?.scrollIntoView()"
            ));
        }
        return window.set_focus();
    }
    let page = match part {
        Some(part) => format!("settings.html#{part}"),
        None => "settings.html".into(),
    };
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App(page.into()))
        .title("Soundscape Desktop settings")
        .inner_size(520.0, 680.0)
        .min_inner_size(380.0, 420.0)
        .build()?;
    Ok(())
}

pub fn open_setup(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETUP) {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, SETUP, WebviewUrl::App("index.html".into()))
        .title("Connect to Soundscape")
        .inner_size(460.0, 520.0)
        .resizable(false)
        .build()?;
    Ok(())
}

fn handle_navigation(app: &AppHandle, target: &Url) -> bool {
    let state = app.state::<AppState>();
    match server::classify(state.server().as_ref(), target) {
        Navigation::Allow => true,
        Navigation::Block => false,
        Navigation::AppSettings => {
            let _ = open_settings(app);
            false
        }
        Navigation::External => {
            links::open(app, target);
            false
        }
        Navigation::Connected { token, user } => {
            let config = state.update(|config| {
                config.token = Some(token);
                config.user = Some(user);
                config.last_notification = None;
            });
            state.set_signed_out(false);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                notify::check(&app).await;
                if let (Some(window), Some(home)) = (
                    app.get_webview_window(MAIN),
                    config.server.and_then(|s| Url::parse(&s).ok()),
                ) {
                    let _ = window.navigate(home);
                }
            });
            false
        }
    }
}

fn open_new_window(app: &AppHandle, target: &Url) {
    let state = app.state::<AppState>();
    match server::classify(state.server().as_ref(), target) {
        Navigation::Allow => {
            if let Some(window) = app.get_webview_window(MAIN) {
                let _ = window.navigate(target.clone());
            }
        }
        Navigation::External => links::open(app, target),
        Navigation::AppSettings => {
            let _ = open_settings(app);
        }
        Navigation::Block | Navigation::Connected { .. } => {}
    }
}

pub fn open_main(app: &AppHandle, start: Url) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN) {
        handover::hand_back(app);
        window.navigate(start)?;
        window.show()?;
        return window.set_focus();
    }
    let handle = app.clone();
    let opener = app.clone();
    let config = app.state::<AppState>().config();
    let mut builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(start));
    let handback = handover::handback_script(app);
    if let Some(script) = &handback {
        builder = builder.initialization_script(script);
    }
    if links::wants_frame_counter(std::env::var("SOUNDSCAPE_SHOW_FPS").ok().as_deref()) {
        builder = builder.initialization_script(links::FRAME_COUNTER);
    }
    builder
        .title("Soundscape")
        .inner_size(1200.0, 820.0)
        .min_inner_size(380.0, 480.0)
        .menu(menu::build(app, &config)?)
        .on_navigation(move |target| handle_navigation(&handle, target))
        .initialization_script(links::SAME_WINDOW)
        .on_new_window(move |target, _| {
            open_new_window(&opener, &target);
            tauri::webview::NewWindowResponse::Deny
        })
        .build()?;
    if handback.is_some() {
        tauri::async_runtime::spawn(handover::take_back(app.clone()));
    }
    tray::refresh(app);
    Ok(())
}

pub fn show(app: &AppHandle) {
    for label in [MAIN, SETUP] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            return;
        }
    }
    let _ = start(app);
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
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let (Some(server), Some(token)) = (
            previous.server.as_deref().and_then(|s| Url::parse(s).ok()),
            previous.token,
        ) {
            server::revoke_token(&server, &token).await;
        }
        if let Some(window) = app.get_webview_window(MAIN) {
            let _ = window.clear_all_browsing_data();
            let _ = window.destroy();
        }
    });
}

fn handle_window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let app = window.app_handle();
    let state = app.state::<AppState>();
    let config = state.config();
    if window.label() == MAIN && stays_running(&config) {
        if state.preview().is_some() {
            api.prevent_close();
            let _ = window.hide();
            if state.preview().is_some_and(|preview| preview.playing) && handover::wanted(app) {
                tauri::async_runtime::spawn(handover::hand_over(app.clone()));
            }
        }
        if !config.told_about_background {
            state.update(|config| config.told_about_background = true);
            let _ = notify::show_notification(
                app,
                "Soundscape is still running",
                "It keeps sending what you play. Open it again from your apps, or quit from the tray or the Soundscape menu.",
            );
        }
    } else if window.label() == SETUP
        && app.get_webview_window(MAIN).is_none()
        && !config.connected()
    {
        app.exit(0);
    }
}

pub fn stays_running(config: &config::Config) -> bool {
    config.connected() && config.keep_running
}

fn handle_run_event(app: &AppHandle, event: RunEvent) {
    if let RunEvent::ExitRequested {
        code: None, api, ..
    } = event
    {
        if stays_running(&app.state::<AppState>().config()) {
            api.prevent_exit();
        }
    }
}

fn start(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<AppState>();
    let config = state.config();
    match state.server() {
        Some(server) if config.connected() => open_main(app, server),
        Some(server) => open_main(app, server::connect_url(&server, &server::device_name())),
        None => open_setup(app),
    }
}

fn main() {
    if let Some(dir) = rendering::config_dir() {
        rendering::prepare(&dir);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|arg| arg == autostart::BACKGROUND_FLAG) {
                show(app);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let config = Config::load(&dir);
            app.manage(AppState {
                config: Mutex::new(config.clone()),
                dir,
                signed_out: AtomicBool::new(false),
                status: Mutex::new(scrobbler::Status::default()),
                friends: Mutex::new(Vec::new()),
                playlists: Mutex::new(Vec::new()),
                loved: Mutex::new(None),
                skip_request: Mutex::new(None),
                skipped: Mutex::new(None),
                today: Mutex::new(None),
                mini: Mutex::new(None),
                preview: Mutex::new(None),
                update: Mutex::new(None),
                native: Mutex::new(None),
            });
            if let Err(error) = tray::create(app.handle(), &config) {
                eprintln!("No tray icon: {error}");
            }
            let background = std::env::args().any(|arg| arg == autostart::BACKGROUND_FLAG);
            if !(background && config.connected()) {
                start(app.handle())?;
            }
            tauri::async_runtime::spawn(scrobbler::run(app.handle().clone()));
            tauri::async_runtime::spawn(notify::run(app.handle().clone()));
            tauri::async_runtime::spawn(friends::run(app.handle().clone()));
            tauri::async_runtime::spawn(music::run(app.handle().clone()));
            tauri::async_runtime::spawn(love::run(app.handle().clone()));
            tauri::async_runtime::spawn(today::run(app.handle().clone()));
            tauri::async_runtime::spawn(yearscape::run(app.handle().clone()));
            tauri::async_runtime::spawn(updates::run(app.handle().clone()));
            tauri::async_runtime::spawn(settings_page::run(app.handle().clone()));
            discord::run(app.handle().clone());
            tauri::async_runtime::spawn(recap::run(app.handle().clone()));
            tauri::async_runtime::spawn(media::run(app.handle().clone()));
            tauri::async_runtime::spawn(search::run(app.handle().clone()));
            Ok(())
        })
        .on_menu_event(|app, event| menu::handle(app, event.id().as_ref()))
        .on_window_event(handle_window_event)
        .invoke_handler(tauri::generate_handler![
            current_server,
            connect_server,
            settings::get_settings,
            settings::set_option,
            settings::set_player,
            settings::add_fix,
            settings::remove_fix,
            settings::set_discord_app,
            mini::get_mini,
            mini::mini_action,
            settings::send_now,
            settings::test_notification,
            settings::settings_action
        ])
        .build(tauri::generate_context!())
        .expect("Soundscape couldn't start")
        .run(handle_run_event);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_running_after_the_window_closes_only_when_connected_and_asked_to() {
        let connected = config::Config {
            server: Some("https://music.example.com/".into()),
            token: Some("t".into()),
            ..config::Config::default()
        };
        assert!(stays_running(&connected));
        assert!(!stays_running(&config::Config {
            keep_running: false,
            ..connected.clone()
        }));
        assert!(!stays_running(&config::Config::default()));
    }
}
