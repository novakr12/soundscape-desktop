use crate::config::Config;
use crate::{
    connect_app, friends, love, mini, music, open_main, open_settings, open_settings_at,
    open_setup, scrobbler, show, sign_out, toggle_pause, tray, AppState, MAIN,
};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager};

const GO_BACK: &str = "go-back";
const GO_FORWARD: &str = "go-forward";
const GO_HOME: &str = "go-home";

pub fn build(app: &AppHandle, config: &Config) -> tauri::Result<Menu<tauri::Wry>> {
    let soundscape = Submenu::with_items(
        app,
        "Soundscape",
        true,
        &[
            &MenuItem::with_id(
                app,
                "version",
                crate::updates::version_text(crate::updates::CURRENT),
                false,
                None::<&str>,
            )?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "reload", "Reload", true, Some("CmdOrCtrl+R"))?,
            &MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?,
            &MenuItem::with_id(app, tray::MINI, "Mini player", true, Some("CmdOrCtrl+M"))?,
            &MenuItem::with_id(
                app,
                tray::CONNECT,
                "Connect this app…",
                !config.connected(),
                None::<&str>,
            )?,
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
            &MenuItem::with_id(
                app,
                tray::PAUSE,
                tray::PAUSE_TEXT,
                config.scrobbling && config.connected(),
                None::<&str>,
            )?,
            &CheckMenuItem::with_id(app, tray::LOVE, tray::LOVE_TEXT, false, false, None::<&str>)?,
            &MenuItem::with_id(app, tray::SKIP, tray::SKIP_TEXT, false, None::<&str>)?,
            &MenuItem::with_id(app, tray::FIX, tray::FIX_TEXT, true, None::<&str>)?,
            &tray::players_menu(app)?,
            &tray::friends_menu(app)?,
            &tray::music_menu(app)?,
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
    let go = Submenu::with_items(
        app,
        "Go",
        true,
        &[
            &MenuItem::with_id(app, GO_BACK, "Back", true, Some("Alt+Left"))?,
            &MenuItem::with_id(app, GO_FORWARD, "Forward", true, Some("Alt+Right"))?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, GO_HOME, "Home", true, Some("Alt+Home"))?,
        ],
    )?;
    Menu::with_items(app, &[&soundscape, &go])
}

fn open_friend(app: &AppHandle, user: &str, play: bool) {
    let state = app.state::<AppState>();
    let Some(server) = state.server() else { return };
    let Some(friend) = state
        .friends()
        .into_iter()
        .find(|friend| friend.user == user)
    else {
        return;
    };
    let target = if play {
        friends::song(&server, &friend)
    } else {
        friends::page(&server, &friend)
    };
    if let Some(target) = target {
        let _ = open_main(app, target);
    }
}

#[derive(Clone, Copy)]
enum Step {
    Back,
    Forward,
}

fn go(app: &AppHandle, step: Step) {
    use webkit2gtk::WebViewExt;
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let _ = window.with_webview(move |webview| {
        let view = webview.inner();
        match step {
            Step::Back if view.can_go_back() => view.go_back(),
            Step::Forward if view.can_go_forward() => view.go_forward(),
            _ => {}
        }
    });
}

fn open_music(app: &AppHandle, playlist: Option<&str>) {
    let Some(server) = app.state::<AppState>().server() else {
        return;
    };
    let target = match playlist {
        Some(id) => music::playlist_page(&server, id),
        None => music::page(&server),
    };
    if let Some(target) = target {
        let _ = open_main(app, target);
    }
}

pub fn handle(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    match id {
        GO_BACK => go(app, Step::Back),
        GO_FORWARD => go(app, Step::Forward),
        GO_HOME => {
            if let Some(server) = state.server() {
                let _ = open_main(app, server);
            }
        }
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
        tray::CONNECT => connect_app(app),
        "tray-open" => show(app),
        tray::SCROBBLING => {
            state.update(|config| config.scrobbling = !config.scrobbling);
            tray::refresh(app);
        }
        "keep-running" => {
            state.update(|config| config.keep_running = !config.keep_running);
        }
        tray::LOVE => {
            tauri::async_runtime::spawn(love::toggle(app.clone()));
        }
        tray::MINI => {
            let _ = mini::open(app);
        }
        tray::PAUSE => {
            toggle_pause(app);
        }
        tray::UPDATE => crate::updates::open_downloads(app),
        tray::FIX => {
            let _ = open_settings_at(app, Some("fixes"));
        }
        tray::SKIP => {
            if let Some((track, _)) = state.status().listening {
                state.request_skip(track);
            }
            tray::refresh(app);
        }
        "quit" => app.exit(0),
        tray::MUSIC_OPEN => open_music(app, None),
        other => {
            if let Some(id) = other.strip_prefix("music-playlist:") {
                open_music(app, Some(id));
            }
            if let Some(user) = other.strip_prefix("friend:") {
                open_friend(app, user, false);
            }
            if let Some(user) = other.strip_prefix("friend-play:") {
                open_friend(app, user, true);
            }
            if let Some(player) = other.strip_prefix("player:") {
                state.update(|config| {
                    let now = scrobbler::enabled(&config.players, player);
                    config.players.insert(player.to_string(), !now);
                });
            }
        }
    }
}
