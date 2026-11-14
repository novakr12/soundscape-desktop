use crate::config::Config;
use crate::scrobbler::{PlayerChoice, Status};
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

const LONGEST: usize = 60;

pub struct Tray {
    listening: MenuItem<Wry>,
    status: MenuItem<Wry>,
    connect: MenuItem<Wry>,
    scrobbling: CheckMenuItem<Wry>,
    players: Submenu<Wry>,
    friends: Submenu<Wry>,
    love: CheckMenuItem<Wry>,
    skip: MenuItem<Wry>,
    pause: MenuItem<Wry>,
}

pub const PLAYERS_MENU: &str = "players-menu";
pub const FRIENDS_MENU: &str = "friends-menu";
pub const LOVE: &str = "love";
pub const SKIP: &str = "skip-scrobble";
pub const LOVE_TEXT: &str = "♥ Love this song";
pub const SKIP_TEXT: &str = "Don't scrobble this one";
pub const PAUSE: &str = "pause-hour";
pub const PAUSE_TEXT: &str = "Pause scrobbling for an hour";
pub const FIX: &str = "fix-title";
pub const FIX_TEXT: &str = "Fix this title…";

#[derive(Debug, PartialEq, Eq)]
pub struct SongActions {
    pub love_text: &'static str,
    pub loved: bool,
    pub can_love: bool,
    pub skip_text: &'static str,
    pub can_skip: bool,
}

pub fn song_actions(status: &Status, loved: Option<bool>, skipped: bool) -> SongActions {
    let playing = status.listening.is_some() && status.connected && !status.signed_out;
    let loved = playing && loved == Some(true);
    let (skip_text, can_skip) = match (playing, status.counted, skipped) {
        (false, _, _) => (SKIP_TEXT, false),
        (true, true, true) => ("Won't be scrobbled", false),
        (true, true, false) => ("Already scrobbled", false),
        (true, false, _) => (SKIP_TEXT, true),
    };
    SongActions {
        love_text: if loved { "♥ Loved" } else { LOVE_TEXT },
        loved,
        can_love: playing,
        skip_text,
        can_skip,
    }
}

fn current_actions(app: &AppHandle, status: &Status) -> SongActions {
    let state = app.state::<crate::AppState>();
    let track = status.listening.as_ref().map(|(track, _)| track);
    song_actions(
        status,
        track.and_then(|track| state.loved_for(track)),
        track.is_some_and(|track| state.was_skipped(track)),
    )
}

fn show_actions(love: &CheckMenuItem<Wry>, skip: &MenuItem<Wry>, actions: &SongActions) {
    let _ = love.set_text(actions.love_text);
    let _ = love.set_checked(actions.loved);
    let _ = love.set_enabled(actions.can_love);
    let _ = skip.set_text(actions.skip_text);
    let _ = skip.set_enabled(actions.can_skip);
}
pub const SCROBBLING: &str = "tray-scrobbling";
pub const CONNECT: &str = "connect-app";

fn short(text: &str) -> String {
    if text.chars().count() <= LONGEST {
        text.to_string()
    } else {
        format!(
            "{}…",
            text.chars()
                .take(LONGEST - 1)
                .collect::<String>()
                .trim_end()
        )
    }
}

pub fn minutes_left(minutes: u64) -> String {
    if minutes == 1 {
        "1 min left".into()
    } else {
        format!("{minutes} min left")
    }
}

pub fn pause_text(status: &Status) -> String {
    match status.paused_minutes {
        Some(minutes) => format!("Resume scrobbling ({})", minutes_left(minutes)),
        None => PAUSE_TEXT.into(),
    }
}

pub fn listening_text(status: &Status) -> String {
    match &status.listening {
        Some((track, _)) => short(&format!("♫ {} — {}", track.title, track.artist)),
        None => "Nothing playing".into(),
    }
}

pub fn status_text(status: &Status, config: &Config) -> String {
    if !config.connected() {
        "Not connected to a server".into()
    } else if status.signed_out {
        "Signed out. Connect again from the window".into()
    } else if !config.scrobbling {
        "Scrobbling is paused".into()
    } else if let Some(minutes) = status.paused_minutes {
        format!("Scrobbling paused · {}", minutes_left(minutes))
    } else {
        match status.waiting {
            0 => "All plays sent".into(),
            1 => "1 play waiting to be sent".into(),
            count => format!("{count} plays waiting to be sent"),
        }
    }
}

pub fn friends_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let empty = MenuItem::with_id(
        app,
        "no-friends",
        "Nobody you follow is playing",
        false,
        None::<&str>,
    )?;
    Submenu::with_id_and_items(app, FRIENDS_MENU, "Friends listening", true, &[&empty])
}

fn fill_friends(app: &AppHandle, menu: &Submenu<Wry>, friends: &[crate::friends::Friend]) {
    if let Ok(items) = menu.items() {
        for item in items {
            let _ = menu.remove(&item);
        }
    }
    if friends.is_empty() {
        if let Ok(empty) = MenuItem::with_id(
            app,
            "no-friends",
            "Nobody you follow is playing",
            false,
            None::<&str>,
        ) {
            let _ = menu.append(&empty);
        }
    }
    for friend in friends {
        if let Ok(item) = MenuItem::with_id(
            app,
            format!("friend:{}", friend.user),
            short(&crate::friends::label(friend)),
            true,
            None::<&str>,
        ) {
            let _ = menu.append(&item);
        }
    }
}

pub fn players_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let empty = MenuItem::with_id(app, "no-players", "No players open", false, None::<&str>)?;
    Submenu::with_id_and_items(app, PLAYERS_MENU, "Players", true, &[&empty])
}

fn fill_players(app: &AppHandle, menu: &Submenu<Wry>, players: &[PlayerChoice]) {
    if let Ok(items) = menu.items() {
        for item in items {
            let _ = menu.remove(&item);
        }
    }
    if players.is_empty() {
        if let Ok(empty) =
            MenuItem::with_id(app, "no-players", "No players open", false, None::<&str>)
        {
            let _ = menu.append(&empty);
        }
    }
    for player in players {
        if let Ok(item) = CheckMenuItem::with_id(
            app,
            format!("player:{}", player.id),
            short(&player.name),
            true,
            player.enabled,
            None::<&str>,
        ) {
            let item: &dyn IsMenuItem<Wry> = &item;
            let _ = menu.append(item);
        }
    }
}

pub fn create(app: &AppHandle, config: &Config) -> tauri::Result<()> {
    let listening = MenuItem::with_id(
        app,
        "tray-listening",
        "Nothing playing",
        false,
        None::<&str>,
    )?;
    let status = MenuItem::with_id(
        app,
        "tray-status",
        status_text(&Status::default(), config),
        false,
        None::<&str>,
    )?;
    let scrobbling = CheckMenuItem::with_id(
        app,
        SCROBBLING,
        "Scrobbling",
        true,
        config.scrobbling,
        None::<&str>,
    )?;
    let connect = MenuItem::with_id(
        app,
        CONNECT,
        "Connect this app…",
        !config.connected(),
        None::<&str>,
    )?;
    let players = players_menu(app)?;
    let friends = friends_menu(app)?;
    let love = CheckMenuItem::with_id(app, LOVE, LOVE_TEXT, false, false, None::<&str>)?;
    let skip = MenuItem::with_id(app, SKIP, SKIP_TEXT, false, None::<&str>)?;
    let fix = MenuItem::with_id(app, FIX, FIX_TEXT, true, None::<&str>)?;
    let pause = MenuItem::with_id(app, PAUSE, PAUSE_TEXT, config.scrobbling, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &listening,
            &love,
            &skip,
            &fix,
            &status,
            &connect,
            &PredefinedMenuItem::separator(app)?,
            &scrobbling,
            &pause,
            &players,
            &friends,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "tray-open", "Open Soundscape", true, None::<&str>)?,
            &MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id("soundscape")
        .tooltip("Soundscape")
        .menu(&menu)
        .show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    if let Ok(cache) = app.path().app_cache_dir() {
        builder = builder.temp_dir_path(cache);
    }
    builder.build(app)?;
    app.manage(Tray {
        listening,
        status,
        connect,
        scrobbling,
        players,
        friends,
        love,
        skip,
        pause,
    });
    Ok(())
}

pub fn refresh(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let status = state.status();
    let config = state.config();
    let friends = state.friends();
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.listening.set_text(listening_text(&status));
        let _ = tray.status.set_text(status_text(&status, &config));
        let _ = tray.scrobbling.set_checked(config.scrobbling);
        let _ = tray
            .connect
            .set_enabled(!config.connected() || status.signed_out);
        fill_players(app, &tray.players, &status.players);
        fill_friends(app, &tray.friends, &friends);
        show_actions(&tray.love, &tray.skip, &current_actions(app, &status));
        let _ = tray.pause.set_text(pause_text(&status));
        let _ = tray
            .pause
            .set_enabled(config.scrobbling && config.connected());
    }
    if let Some(icon) = app.tray_by_id("soundscape") {
        let tooltip = match &status.listening {
            Some((track, player)) => format!("{} — {} ({player})", track.title, track.artist),
            None => "Soundscape".into(),
        };
        let _ = icon.set_tooltip(Some(short(&tooltip)));
    }
    let Some(window) = app.get_webview_window(crate::MAIN) else {
        return;
    };
    let Some(menu) = window.menu() else { return };
    let Ok(items) = menu.items() else { return };
    for item in items {
        let Some(submenu) = item.as_submenu() else {
            continue;
        };
        if let Some(check) = submenu
            .get(SCROBBLING)
            .and_then(|item| item.as_check_menuitem().cloned())
        {
            let _ = check.set_checked(config.scrobbling);
        }
        if let Some(connect) = submenu
            .get(CONNECT)
            .and_then(|item| item.as_menuitem().cloned())
        {
            let _ = connect.set_enabled(!config.connected() || status.signed_out);
        }
        if let Some(players) = submenu
            .get(PLAYERS_MENU)
            .and_then(|item| item.as_submenu().cloned())
        {
            fill_players(app, &players, &status.players);
        }
        if let Some(pause) = submenu
            .get(PAUSE)
            .and_then(|item| item.as_menuitem().cloned())
        {
            let _ = pause.set_text(pause_text(&status));
            let _ = pause.set_enabled(config.scrobbling && config.connected());
        }
        if let (Some(love), Some(skip)) = (
            submenu
                .get(LOVE)
                .and_then(|item| item.as_check_menuitem().cloned()),
            submenu
                .get(SKIP)
                .and_then(|item| item.as_menuitem().cloned()),
        ) {
            show_actions(&love, &skip, &current_actions(app, &status));
        }
        if let Some(menu) = submenu
            .get(FRIENDS_MENU)
            .and_then(|item| item.as_submenu().cloned())
        {
            fill_friends(app, &menu, &friends);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobble::Track;

    fn connected() -> Config {
        Config {
            server: Some("https://a.example/".into()),
            user: Some("u".into()),
            token: Some("t".into()),
            ..Config::default()
        }
    }

    #[test]
    fn offers_love_and_skip_only_while_a_song_plays() {
        let mut status = Status {
            connected: true,
            ..Status::default()
        };
        let idle = song_actions(&status, None, false);
        assert!(!idle.can_love && !idle.can_skip);
        status.listening = Some((
            Track {
                artist: "Air".into(),
                title: "Alone in Kyoto".into(),
                album: None,
                length_ms: None,
            },
            "Strawberry".into(),
        ));
        let fresh = song_actions(&status, None, false);
        assert_eq!(
            (fresh.love_text, fresh.loved, fresh.can_love),
            (LOVE_TEXT, false, true)
        );
        assert_eq!((fresh.skip_text, fresh.can_skip), (SKIP_TEXT, true));
        assert_eq!(
            song_actions(&status, Some(true), false).love_text,
            "♥ Loved"
        );
        status.counted = true;
        assert_eq!(
            song_actions(&status, None, true).skip_text,
            "Won't be scrobbled"
        );
        assert_eq!(
            song_actions(&status, None, false).skip_text,
            "Already scrobbled"
        );
        assert!(!song_actions(&status, None, false).can_skip);
        status.signed_out = true;
        assert!(!song_actions(&status, Some(true), false).can_love);
    }

    #[test]
    fn says_what_is_playing_in_a_short_line() {
        let mut status = Status::default();
        assert_eq!(listening_text(&status), "Nothing playing");
        status.listening = Some((
            Track {
                artist: "Radiohead".into(),
                title: "Airbag".into(),
                album: None,
                length_ms: None,
            },
            "Strawberry".into(),
        ));
        assert_eq!(listening_text(&status), "♫ Airbag — Radiohead");
        status.listening = Some((
            Track {
                artist: "A".repeat(80),
                title: "T".into(),
                album: None,
                length_ms: None,
            },
            "x".into(),
        ));
        assert_eq!(listening_text(&status).chars().count(), LONGEST);
    }

    #[test]
    fn explains_the_state_of_scrobbling() {
        let mut status = Status::default();
        assert_eq!(
            status_text(&status, &Config::default()),
            "Not connected to a server"
        );
        assert_eq!(status_text(&status, &connected()), "All plays sent");
        status.waiting = 1;
        assert_eq!(
            status_text(&status, &connected()),
            "1 play waiting to be sent"
        );
        status.waiting = 12;
        assert_eq!(
            status_text(&status, &connected()),
            "12 plays waiting to be sent"
        );
        let paused = Config {
            scrobbling: false,
            ..connected()
        };
        assert_eq!(status_text(&status, &paused), "Scrobbling is paused");
        status.signed_out = true;
        assert!(status_text(&status, &connected()).starts_with("Signed out"));
    }
}
