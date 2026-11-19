use crate::host::{AppHandle, Manager};
use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
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
    crate::server::get_json::<Answer>(server, token, "/api/friends/listening", &[])
        .await
        .map(|answer| answer.listening)
}

pub async fn run(app: AppHandle) {
    crate::server::poll(app, FIRST, EVERY, |app| async move { check(&app).await }).await;
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let friends = match crate::server::credentials(app) {
        Some((server, token)) => fetch(&server, &token).await.unwrap_or_default(),
        None => Vec::new(),
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
