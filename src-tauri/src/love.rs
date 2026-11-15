use crate::scrobble::Track;
use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

const EVERY: Duration = Duration::from_secs(3);

#[derive(Debug, Deserialize)]
struct Answer {
    loved: bool,
}

async fn fetch(server: &Url, token: &str, track: &Track) -> Result<bool, Failure> {
    let query = [
        ("artist", track.artist.as_str()),
        ("title", track.title.as_str()),
    ];
    crate::server::get_json::<Answer>(server, token, "/api/love", &query)
        .await
        .map(|answer| answer.loved)
}

async fn send(server: &Url, token: &str, track: &Track, loved: bool) -> Result<(), Failure> {
    let request = crate::server::client()
        .post(crate::server::address(server, "/api/love")?)
        .json(&serde_json::json!({ "artist": track.artist, "title": track.title, "loved": loved }));
    crate::server::answer(request, token).await.map(|_| ())
}

pub async fn run(app: AppHandle) {
    loop {
        tokio::time::sleep(EVERY).await;
        let state = app.state::<AppState>();
        let Some((track, _)) = state.status().listening else {
            continue;
        };
        if state.loved_for(&track).is_some() {
            continue;
        }
        let Some((server, token)) = crate::server::credentials(&app) else {
            continue;
        };
        let loved = match fetch(&server, &token, &track).await {
            Ok(loved) => loved,
            Err(Failure::Offline) => continue,
            Err(_) => false,
        };
        state.set_loved(&track, loved);
        crate::tray::refresh(&app);
    }
}

pub async fn toggle(app: AppHandle) {
    let state = app.state::<AppState>();
    let Some((track, _)) = state.status().listening else {
        return;
    };
    let Some((server, token)) = crate::server::credentials(&app) else {
        return;
    };
    let before = state.loved_for(&track).unwrap_or(false);
    state.set_loved(&track, !before);
    crate::tray::refresh(&app);
    if let Err(error) = send(&server, &token, &track, !before).await {
        eprintln!("Couldn't change the love for {}: {error:?}", track.title);
        state.set_loved(&track, before);
        crate::tray::refresh(&app);
    }
}
