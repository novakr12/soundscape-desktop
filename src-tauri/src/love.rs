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

fn credentials(app: &AppHandle) -> Option<(Url, String)> {
    let state = app.state::<AppState>();
    if state.signed_out() {
        return None;
    }
    let config = state.config();
    Some((Url::parse(config.server.as_deref()?).ok()?, config.token?))
}

async fn fetch(server: &Url, token: &str, track: &Track) -> Result<bool, Failure> {
    let mut address = server.join("/api/love").map_err(|_| Failure::Refused(0))?;
    address
        .query_pairs_mut()
        .append_pair("artist", &track.artist)
        .append_pair("title", &track.title);
    let response = crate::server::client()
        .get(address)
        .header("authorization", format!("Token {token}"))
        .send()
        .await
        .map_err(|_| Failure::Offline)?;
    match response.status().as_u16() {
        200..=299 => response
            .json::<Answer>()
            .await
            .map(|answer| answer.loved)
            .map_err(|_| Failure::Refused(200)),
        401 | 403 => Err(Failure::SignedOut),
        status => Err(Failure::Refused(status)),
    }
}

async fn send(server: &Url, token: &str, track: &Track, loved: bool) -> Result<(), Failure> {
    let address = server.join("/api/love").map_err(|_| Failure::Refused(0))?;
    let response = crate::server::client()
        .post(address)
        .header("authorization", format!("Token {token}"))
        .json(&serde_json::json!({ "artist": track.artist, "title": track.title, "loved": loved }))
        .send()
        .await
        .map_err(|_| Failure::Offline)?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(Failure::SignedOut),
        status => Err(Failure::Refused(status)),
    }
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
        let Some((server, token)) = credentials(&app) else {
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
    let Some((server, token)) = credentials(&app) else {
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
