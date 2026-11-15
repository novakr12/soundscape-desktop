use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

const EVERY: Duration = Duration::from_secs(300);
const FIRST: Duration = Duration::from_secs(8);
const MOST: usize = 30;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "songCount")]
    pub song_count: u64,
}

#[derive(Debug, Deserialize)]
struct Answer {
    playlists: Vec<Playlist>,
}

pub fn label(playlist: &Playlist) -> String {
    match playlist.song_count {
        1 => format!("{} · 1 song", playlist.name),
        count => format!("{} · {count} songs", playlist.name),
    }
}

pub fn page(server: &Url) -> Option<Url> {
    let target = server.join("/music").ok()?;
    (target.origin() == server.origin()).then_some(target)
}

pub fn playlist_page(server: &Url, id: &str) -> Option<Url> {
    let mut target = page(server)?;
    target
        .query_pairs_mut()
        .append_pair("playlist", id)
        .append_pair("play", "1");
    Some(target)
}

pub async fn fetch(server: &Url, token: &str) -> Result<Vec<Playlist>, Failure> {
    crate::server::get_json::<Answer>(server, token, "/api/music/playlists", &[])
        .await
        .map(|answer| answer.playlists.into_iter().take(MOST).collect())
}

pub async fn run(app: AppHandle) {
    crate::server::poll(app, FIRST, EVERY, |app| async move { check(&app).await }).await;
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let found = match crate::server::credentials(app) {
        Some((server, token)) => fetch(&server, &token).await,
        None => Ok(Vec::new()),
    };
    let playlists = match found {
        Ok(playlists) => playlists,
        Err(Failure::Offline) => return,
        Err(_) => Vec::new(),
    };
    if state.set_playlists(playlists) {
        crate::tray::refresh(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_playlist_with_its_size() {
        let playlist = Playlist {
            id: "pl-1".into(),
            name: "Evening".into(),
            song_count: 12,
        };
        assert_eq!(label(&playlist), "Evening · 12 songs");
        assert_eq!(
            label(&Playlist {
                song_count: 1,
                ..playlist
            }),
            "Evening · 1 song"
        );
    }

    #[test]
    fn opens_the_music_tab_on_your_own_server() {
        let server = Url::parse("https://music.example.com/soundscape/").unwrap();
        assert_eq!(
            page(&server).unwrap().as_str(),
            "https://music.example.com/music"
        );
        assert_eq!(
            playlist_page(&server, "a b&c=d").unwrap().as_str(),
            "https://music.example.com/music?playlist=a+b%26c%3Dd&play=1"
        );
    }

    #[test]
    fn reads_the_playlists_the_server_sends() {
        let answer: Answer = serde_json::from_str(
            r#"{"connected":true,"playlists":[{"id":"pl-1","name":"Evening","songCount":2},{"id":"pl-2","name":"Empty"}]}"#,
        )
        .unwrap();
        assert_eq!(answer.playlists[0].song_count, 2);
        assert_eq!(answer.playlists[1].song_count, 0);
    }
}
