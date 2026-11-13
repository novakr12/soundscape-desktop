mod autostart;
mod config;
mod mpris;
mod notify;
mod queue;
mod scrobble;
mod scrobbler;
mod server;
mod settings;
mod submit;
mod tray;

use config::Config;
use server::Navigation;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use url::Url;

pub const MAIN: &str = "main";
const SETUP: &str = "setup";
const SETTINGS: &str = "settings";

pub struct AppState {
    dir: PathBuf,
    config: Mutex<Config>,
    signed_out: AtomicBool,
    status: Mutex<scrobbler::Status>,
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

    fn server(&self) -> Option<Url> {
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
    if let Some(window) = app.get_webview_window(SETTINGS) {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("Soundscape settings")
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

fn menu(app: &AppHandle, config: &Config) -> tauri::Result<Menu<tauri::Wry>> {
    let soundscape = Submenu::with_items(
        app,
        "Soundscape",
        true,
        &[
            &MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?,
            &MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?,
            &MenuItem::with_id(app, "change-server", "Change server…", true, None::<&str>)?,
            &MenuItem::with_id(app, "sign-out", "Sign out", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &CheckMenuItem::with_id(
                app,
                tray::SCROBBLING,
                "Scrobbling",
                true,
                config.scrobbling,
                None::<&str>,
            )?,
            &tray::players_menu(app)?,
            &CheckMenuItem::with_id(
                app,
                "keep-running",
                "Keep running when closed",
                true,
                config.keep_running,
                None::<&str>,
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;
    Menu::with_items(app, &[&soundscape])
}

fn handle_navigation(app: &AppHandle, target: &Url) -> bool {
    let state = app.state::<AppState>();
    match server::classify(state.server().as_ref(), target) {
        Navigation::Allow => true,
        Navigation::Block => false,
        Navigation::External => {
            if let Err(error) = app.opener().open_url(target.as_str(), None::<&str>) {
                eprintln!("Couldn't open {target}: {error}");
            }
            false
        }
        Navigation::Connected { token, user } => {
            let config = state.update(|config| {
                config.token = Some(token);
                config.user = Some(user);
            });
            state.set_signed_out(false);
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
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

fn open_main(app: &AppHandle, start: Url) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(MAIN) {
        window.navigate(start)?;
        window.show()?;
        return window.set_focus();
    }
    let handle = app.clone();
    let config = app.state::<AppState>().config();
    WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(start))
        .title("Soundscape")
        .inner_size(1200.0, 820.0)
        .min_inner_size(380.0, 480.0)
        .menu(menu(app, &config)?)
        .on_navigation(move |target| handle_navigation(&handle, target))
        .build()?;
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

fn handle_menu(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    match id {
        "reload" => {
            if let Some(window) = app.get_webview_window(MAIN) {
                let _ = window.eval("location.reload()");
            }
        }
        "change-server" => {
            let _ = open_setup(app);
        }
        "settings" => {
            let _ = open_settings(app);
        }
        "sign-out" => sign_out(app),
        "tray-open" => show(app),
        tray::SCROBBLING => {
            state.update(|config| config.scrobbling = !config.scrobbling);
            tray::refresh(app);
        }
        "keep-running" => {
            state.update(|config| config.keep_running = !config.keep_running);
        }
        "quit" => app.exit(0),
        other => {
            if let Some(player) = other.strip_prefix("player:") {
                state.update(|config| {
                    let now = scrobbler::enabled(&config.players, player);
                    config.players.insert(player.to_string(), !now);
                });
            }
        }
    }
}

fn handle_window_event(window: &Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let app = window.app_handle();
    let state = app.state::<AppState>();
    let config = state.config();
    if window.label() == MAIN && config.connected() && config.keep_running {
        api.prevent_close();
        let _ = window.hide();
        if !config.told_about_background {
            state.update(|config| config.told_about_background = true);
            let _ = app
                .notification()
                .builder()
                .title("Soundscape is still running")
                .body("It keeps sending what you play. Open it again from your apps, or quit from the tray or the Soundscape menu.")
                .show();
        }
    } else if window.label() == SETUP
        && app.get_webview_window(MAIN).is_none()
        && !config.connected()
    {
        app.exit(0);
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
            Ok(())
        })
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .on_window_event(handle_window_event)
        .invoke_handler(tauri::generate_handler![
            current_server,
            connect_server,
            settings::get_settings,
            settings::set_option,
            settings::set_player,
            settings::send_now,
            settings::settings_action
        ])
        .run(tauri::generate_context!())
        .expect("Soundscape couldn't start");
}
