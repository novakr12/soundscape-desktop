use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

const EVERY: Duration = Duration::from_secs(45);
const FIRST: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Friend {
    pub user: String,
    pub title: String,
    pub artist: String,
    pub url: String,
    #[serde(default, rename = "trackId")]
    pub track_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Answer {
    listening: Vec<Friend>,
}

pub fn label(friend: &Friend) -> String {
    format!("{} — {} · {}", friend.user, friend.title, friend.artist)
}

pub fn page(server: &Url, friend: &Friend) -> Option<Url> {
    let target = server.join(&friend.url).ok()?;
    (target.origin() == server.origin()).then_some(target)
}

pub fn song(server: &Url, friend: &Friend) -> Option<Url> {
    let mut target = server.join("/").ok()?;
    target
        .path_segments_mut()
        .ok()?
        .clear()
        .extend(["track", friend.track_id.as_deref()?]);
    target.set_query(Some("play=1"));
    Some(target)
}

pub async fn fetch(server: &Url, token: &str) -> Result<Vec<Friend>, Failure> {
    let address = server
        .join("/api/friends/listening")
        .map_err(|_| Failure::Refused(0))?;
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
            .map(|answer| answer.listening)
            .map_err(|_| Failure::Refused(200)),
        401 | 403 => Err(Failure::SignedOut),
        404 => Err(Failure::Refused(404)),
        _ => Err(Failure::Offline),
    }
}

pub async fn run(app: AppHandle) {
    tokio::time::sleep(FIRST).await;
    loop {
        check(&app).await;
        tokio::time::sleep(EVERY).await;
    }
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config();
    let friends = match (
        config.server.as_deref().and_then(|s| Url::parse(s).ok()),
        config.token.clone(),
    ) {
        (Some(server), Some(token)) if !state.signed_out() => {
            fetch(&server, &token).await.unwrap_or_default()
        }
        _ => Vec::new(),
    };
    if state.set_friends(friends) {
        crate::tray::refresh(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(url: &str) -> Friend {
        Friend {
            user: "anna".into(),
            title: "Teardrop".into(),
            artist: "Massive Attack".into(),
            url: url.into(),
            track_id: Some("massive-attack-teardrop".into()),
        }
    }

    #[test]
    fn describes_a_friend_in_one_line() {
        assert_eq!(
            label(&friend("/u/anna")),
            "anna — Teardrop · Massive Attack"
        );
    }

    #[test]
    fn opens_only_pages_on_your_own_server() {
        let server = Url::parse("https://music.example.com/").unwrap();
        assert_eq!(
            page(&server, &friend("/u/anna")).unwrap().as_str(),
            "https://music.example.com/u/anna"
        );
        assert_eq!(
            page(&server, &friend("https://evil.example.org/u/anna")),
            None
        );
        assert_eq!(page(&server, &friend("//evil.example.org/x")), None);
    }

    #[test]
    fn plays_the_song_on_your_own_server() {
        let server = Url::parse("https://music.example.com/").unwrap();
        assert_eq!(
            song(&server, &friend("/u/anna")).unwrap().as_str(),
            "https://music.example.com/track/massive-attack-teardrop?play=1"
        );
        let odd = Friend {
            track_id: Some("../admin?x=1#y".into()),
            ..friend("/u/anna")
        };
        assert_eq!(
            song(&server, &odd).unwrap().as_str(),
            "https://music.example.com/track/..%2Fadmin%3Fx=1%23y?play=1"
        );
        let old_server = Friend {
            track_id: None,
            ..friend("/u/anna")
        };
        assert_eq!(song(&server, &old_server), None);
    }
}
