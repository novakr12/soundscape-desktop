use crate::media::Preview;
use crate::native::{self, Player};
use crate::{AppState, MAIN};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

static HANDBACKS: AtomicU64 = AtomicU64::new(0);

fn with_player<T>(app: &AppHandle, read: impl FnOnce(&Player) -> T) -> Option<T> {
    let state = app.state::<AppState>();
    let player = state.native.lock().expect("native lock");
    player.as_ref().map(read)
}

pub fn active(app: &AppHandle) -> bool {
    with_player(app, |_| ()).is_some()
}

pub fn preview(app: &AppHandle) -> Option<Preview> {
    with_player(app, Player::preview).flatten()
}

pub fn control(app: &AppHandle, action: &str, seconds: f64) -> bool {
    let Some(command) = native::command_for(action, seconds) else {
        return active(app) && matches!(action, "shuffle");
    };
    with_player(app, |player| player.send(command)).is_some()
}

pub fn wanted(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let config = state.config();
    config.native_player && config.connected() && !state.signed_out()
}

fn report(server: Url, token: String) -> native::Report {
    Box::new(move |kind, track| {
        let server = server.clone();
        let token = token.clone();
        tauri::async_runtime::spawn(async move {
            let path = if kind == "play" {
                "/api/play"
            } else {
                "/api/now-playing"
            };
            let Ok(address) = crate::server::address(&server, path) else {
                return;
            };
            let mut body = serde_json::json!({ "trackId": track.track_id });
            if let Some(song) = &track.song_id {
                body["songId"] = serde_json::Value::from(song.clone());
            }
            let _ =
                crate::server::answer(crate::server::client().post(address).json(&body), &token)
                    .await;
        });
    })
}

pub async fn hand_over(app: AppHandle) {
    let Some((server, token)) = crate::server::credentials(&app) else {
        return;
    };
    let Some(handover) = crate::media::evaluate(&app, native::READ_HANDOVER)
        .await
        .and_then(|json| native::parse_handover(&json))
    else {
        return;
    };
    if !handover.playing {
        return;
    }
    let _ = crate::media::evaluate(&app, native::PAUSE_PAGE).await;
    let player = match native::start(
        server.clone(),
        token.clone(),
        handover,
        report(server, token),
    ) {
        Ok(player) => player,
        Err(error) => {
            eprintln!("The built-in player couldn't start: {error}");
            let _ = crate::media::evaluate(&app, native::RESUME_PAGE).await;
            return;
        }
    };
    *app.state::<AppState>().native.lock().expect("native lock") = Some(player);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    if window.is_visible().unwrap_or(true) {
        hand_back(&app);
        let _ = crate::media::evaluate(&app, native::RESUME_PAGE).await;
        return;
    }
    let _ = window.destroy();
}

pub fn hand_back(app: &AppHandle) -> Option<String> {
    let player = app
        .state::<AppState>()
        .native
        .lock()
        .expect("native lock")
        .take()?;
    let resume = player.resume();
    drop(player);
    Some(native::handback_script(
        &resume,
        HANDBACKS.fetch_add(1, Ordering::Relaxed) + crate::config::now_seconds() as u64,
    ))
}
