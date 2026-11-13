use crate::config::Config;
use crate::scrobbler::{PlayerChoice, Status};
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

const LONGEST: usize = 60;

pub struct Tray {
    listening: MenuItem<Wry>,
    status: MenuItem<Wry>,
    scrobbling: CheckMenuItem<Wry>,
    players: Submenu<Wry>,
}

pub const PLAYERS_MENU: &str = "players-menu";
pub const SCROBBLING: &str = "tray-scrobbling";

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
    } else {
        match status.waiting {
            0 => "All plays sent".into(),
            1 => "1 play waiting to be sent".into(),
            count => format!("{count} plays waiting to be sent"),
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
    let players = players_menu(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &listening,
            &status,
            &PredefinedMenuItem::separator(app)?,
            &scrobbling,
            &players,
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
    builder.build(app)?;
    app.manage(Tray {
        listening,
        status,
        scrobbling,
        players,
    });
    Ok(())
}

pub fn refresh(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let status = state.status();
    let config = state.config();
    if let Some(tray) = app.try_state::<Tray>() {
        let _ = tray.listening.set_text(listening_text(&status));
        let _ = tray.status.set_text(status_text(&status, &config));
        let _ = tray.scrobbling.set_checked(config.scrobbling);
        fill_players(app, &tray.players, &status.players);
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
        if let Some(players) = submenu
            .get(PLAYERS_MENU)
            .and_then(|item| item.as_submenu().cloned())
        {
            fill_players(app, &players, &status.players);
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
