mod config;
mod server;

use config::Config;
use server::Navigation;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use url::Url;

const MAIN: &str = "main";
const SETUP: &str = "setup";

struct AppState {
    dir: PathBuf,
    config: Mutex<Config>,
}

impl AppState {
    fn config(&self) -> Config {
        self.config.lock().expect("config lock").clone()
    }

    fn update(&self, change: impl FnOnce(&mut Config)) -> Config {
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
    let same = previous.server.as_deref() == Some(server.as_str()) && previous.connected();
    if !same {
        state.update(|config| {
            config.server = Some(server.to_string());
            config.user = None;
            config.token = None;
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

fn open_setup(app: &AppHandle) -> tauri::Result<()> {
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

fn menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let soundscape = Submenu::with_items(
        app,
        "Soundscape",
        true,
        &[
            &MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?,
            &MenuItem::with_id(app, "change-server", "Change server…", true, None::<&str>)?,
            &MenuItem::with_id(app, "sign-out", "Sign out", true, None::<&str>)?,
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
    WebviewWindowBuilder::new(app, MAIN, WebviewUrl::External(start))
        .title("Soundscape")
        .inner_size(1200.0, 820.0)
        .min_inner_size(380.0, 480.0)
        .menu(menu(app)?)
        .on_navigation(move |target| handle_navigation(&handle, target))
        .build()?;
    Ok(())
}

fn sign_out(app: &AppHandle) {
    let state = app.state::<AppState>();
    let previous = state.config();
    state.update(|config| {
        config.token = None;
        config.user = None;
    });
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
            let _ = window.close();
        }
        let _ = open_setup(&app);
    });
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        "reload" => {
            if let Some(window) = app.get_webview_window(MAIN) {
                let _ = window.eval("location.reload()");
            }
        }
        "change-server" => {
            let _ = open_setup(app);
        }
        "sign-out" => sign_out(app),
        "quit" => app.exit(0),
        _ => {}
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
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            app.manage(AppState {
                config: Mutex::new(Config::load(&dir)),
                dir,
            });
            start(app.handle())?;
            Ok(())
        })
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .invoke_handler(tauri::generate_handler![current_server, connect_server])
        .run(tauri::generate_context!())
        .expect("Soundscape couldn't start");
}
