use crate::queue::Queue;
use crate::scrobbler::{self, enabled};
use crate::submit::Failure;
use crate::{autostart, tray, AppState};
use serde::Serialize;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PlayerView {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub open: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SongView {
    pub artist: String,
    pub title: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FixView {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct SettingsView {
    pub server: Option<String>,
    pub user: Option<String>,
    pub connected: bool,
    pub signed_out: bool,
    pub scrobbling: bool,
    pub notifications: bool,
    pub weekly_recap: bool,
    pub streak_reminder: bool,
    pub yearscape_notification: bool,
    pub update_notice: bool,
    pub discord: bool,
    pub native_player: bool,
    pub discord_app_id: Option<String>,
    pub keep_running: bool,
    pub autostart: bool,
    pub flatpak: bool,
    pub waiting: usize,
    pub listening: Option<String>,
    pub players: Vec<PlayerView>,
    pub playing: Option<SongView>,
    pub sent_as: Option<SongView>,
    pub paused_minutes: Option<u64>,
    pub fixes: Vec<FixView>,
}

pub fn players(
    open: &[scrobbler::PlayerChoice],
    chosen: &std::collections::BTreeMap<String, bool>,
) -> Vec<PlayerView> {
    let mut list: Vec<PlayerView> = open
        .iter()
        .map(|player| PlayerView {
            id: player.id.clone(),
            name: player.name.clone(),
            enabled: enabled(chosen, &player.id),
            open: true,
        })
        .collect();
    for id in chosen.keys() {
        if !list.iter().any(|player| &player.id == id) {
            list.push(PlayerView {
                id: id.clone(),
                name: id.clone(),
                enabled: enabled(chosen, id),
                open: false,
            });
        }
    }
    list.sort_by(|a, b| {
        b.open
            .cmp(&a.open)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    list
}

pub fn view(app: &AppHandle) -> SettingsView {
    let state = app.state::<AppState>();
    let config = state.config();
    let status = state.status();
    SettingsView {
        server: config
            .server
            .clone()
            .map(|server| server.trim_end_matches('/').to_string()),
        user: config.user.clone(),
        connected: config.connected(),
        signed_out: state.signed_out(),
        scrobbling: config.scrobbling,
        notifications: config.notifications,
        weekly_recap: config.weekly_recap,
        streak_reminder: config.streak_reminder,
        yearscape_notification: config.yearscape_notification,
        update_notice: config.update_notice,
        discord: config.discord,
        native_player: config.native_player,
        discord_app_id: config.discord_app_id.clone(),
        keep_running: config.keep_running,
        autostart: config.autostart,
        flatpak: autostart::in_flatpak(),
        waiting: Queue::new(&state.dir).len(),
        listening: status
            .listening
            .as_ref()
            .map(|(track, player)| format!("{} — {} ({player})", track.title, track.artist)),
        players: players(&status.players, &config.players),
        playing: status.original.as_ref().map(|track| SongView {
            artist: track.artist.clone(),
            title: track.title.clone(),
        }),
        paused_minutes: config.paused_minutes(crate::config::now_seconds()),
        sent_as: match (&status.original, &status.listening) {
            (Some(original), Some((sent, _)))
                if original.artist != sent.artist || original.title != sent.title =>
            {
                Some(SongView {
                    artist: sent.artist.clone(),
                    title: sent.title.clone(),
                })
            }
            _ => None,
        },
        fixes: config
            .fixes
            .iter()
            .map(|fix| {
                let (from, to) = crate::fixes::describe(fix);
                FixView { from, to }
            })
            .collect(),
    }
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsView {
    view(&app)
}

#[tauri::command]
pub async fn set_option(app: AppHandle, name: String, value: bool) -> Result<SettingsView, String> {
    let state = app.state::<AppState>();
    match name.as_str() {
        "scrobbling" => {
            state.update(|config| config.scrobbling = value);
        }
        "notifications" => {
            state.update(|config| config.notifications = value);
        }
        "yearscape_notification" => {
            state.update(|config| config.yearscape_notification = value);
        }
        "update_notice" => {
            state.update(|config| config.update_notice = value);
            if !value && state.set_update(None) {
                crate::tray::refresh(&app);
            }
        }
        "native_player" => {
            state.update(|config| config.native_player = value);
        }
        "discord" => {
            state.update(|config| config.discord = value);
        }
        "streak_reminder" => {
            state.update(|config| config.streak_reminder = value);
        }
        "weekly_recap" => {
            state.update(|config| config.weekly_recap = value);
        }
        "keep_running" => {
            state.update(|config| config.keep_running = value);
        }
        "autostart" => {
            if !autostart::set(value).await? {
                return Err(
                    "Your desktop didn't allow Soundscape to start when you log in.".into(),
                );
            }
            state.update(|config| config.autostart = value);
        }
        _ => return Err(format!("Unknown setting {name}")),
    }
    tray::refresh(&app);
    Ok(view(&app))
}

#[tauri::command]
pub fn add_fix(
    app: AppHandle,
    artist: String,
    title: String,
    new_artist: String,
    new_title: String,
    every_song: bool,
) -> Result<SettingsView, String> {
    let fix = crate::fixes::make(&artist, &title, &new_artist, &new_title, every_song)?;
    app.state::<AppState>()
        .update(|config| crate::fixes::add(&mut config.fixes, fix));
    Ok(view(&app))
}

#[tauri::command]
pub fn set_discord_app(app: AppHandle, app_id: String) -> Result<SettingsView, String> {
    let app_id = app_id.trim().to_string();
    if !app_id.is_empty() && !crate::discord::valid_app_id(&app_id) {
        return Err("That doesn't look like a Discord application ID. It's a long number.".into());
    }
    app.state::<AppState>()
        .update(|config| config.discord_app_id = (!app_id.is_empty()).then_some(app_id));
    Ok(view(&app))
}

#[tauri::command]
pub fn remove_fix(app: AppHandle, index: usize) -> SettingsView {
    app.state::<AppState>().update(|config| {
        if index < config.fixes.len() {
            config.fixes.remove(index);
        }
    });
    view(&app)
}

#[tauri::command]
pub fn set_player(app: AppHandle, id: String, value: bool) -> SettingsView {
    app.state::<AppState>().update(|config| {
        config.players.insert(id, value);
    });
    view(&app)
}

#[tauri::command]
pub async fn send_now(app: AppHandle) -> Result<SettingsView, String> {
    match scrobbler::send_waiting(&app).await {
        Ok(_) => {
            tray::refresh(&app);
            Ok(view(&app))
        }
        Err(Failure::Offline) => {
            Err("Couldn't reach the server. The plays are sent as soon as it's back.".into())
        }
        Err(Failure::SignedOut) => Err(
            "The server doesn't accept this app's token any more. Sign out and connect again."
                .into(),
        ),
        Err(Failure::Refused(_)) => Ok(view(&app)),
    }
}

#[tauri::command]
pub fn test_notification(app: AppHandle) -> Result<(), String> {
    crate::notify::show_notification(
        &app,
        "Soundscape",
        "Notifications work. New followers and milestones show up like this.",
    )
    .map_err(|error| format!("Your desktop didn't show the notification: {error}"))
}

#[tauri::command]
pub fn settings_action(app: AppHandle, action: String) {
    match action.as_str() {
        "change-server" => {
            let _ = crate::open_setup(&app);
        }
        "sign-out" => crate::sign_out(&app),
        "connect" => crate::connect_app(&app),
        "open" => crate::show(&app),
        "pause" => crate::toggle_pause(&app),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobbler::PlayerChoice;
    use std::collections::BTreeMap;

    #[test]
    fn lists_open_players_first_and_remembers_closed_ones() {
        let open = vec![
            PlayerChoice {
                id: "spotify".into(),
                name: "Spotify".into(),
                enabled: true,
            },
            PlayerChoice {
                id: "firefox".into(),
                name: "Firefox".into(),
                enabled: false,
            },
        ];
        let mut chosen = BTreeMap::new();
        chosen.insert("rhythmbox".to_string(), false);
        chosen.insert("firefox".to_string(), true);
        let list = players(&open, &chosen);
        assert_eq!(
            list.iter()
                .map(|p| (p.name.as_str(), p.enabled, p.open))
                .collect::<Vec<_>>(),
            vec![
                ("Firefox", true, true),
                ("Spotify", true, true),
                ("rhythmbox", false, false)
            ]
        );
    }
}
