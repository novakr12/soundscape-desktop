use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use url::Url;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{interface, Connection};

pub const BUS_NAME: &str = "io.github.novakr12.SoundscapeDesktop.SearchProvider";
const PATH: &str = "/io/github/novakr12/SoundscapeDesktop/SearchProvider";
const ICON: &str = "io.github.novakr12.SoundscapeDesktop";
const SHORTEST: usize = 2;
const EXACT_MATCH: i32 = 100;
const POSSIBLE_MATCH: i32 = 30;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Artist {
    id: String,
    name: String,
    plays: u64,
    url: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Song {
    id: String,
    title: String,
    artist: String,
    plays: u64,
    url: String,
}

#[derive(Debug, Deserialize)]
struct Answer {
    artists: Vec<Artist>,
    songs: Vec<Song>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub name: String,
    pub description: String,
    pub url: String,
}

fn plays(count: u64) -> String {
    if count == 1 {
        "1 play".into()
    } else {
        format!("{count} plays")
    }
}

fn hits(answer: Answer) -> Vec<Hit> {
    let artists = answer.artists.into_iter().map(|artist| Hit {
        id: format!("artist:{}", artist.id),
        name: artist.name,
        description: format!("Artist · {}", plays(artist.plays)),
        url: artist.url,
    });
    let songs = answer.songs.into_iter().map(|song| Hit {
        id: format!("song:{}", song.id),
        name: song.title,
        description: format!("{} · {}", song.artist, plays(song.plays)),
        url: song.url,
    });
    artists.chain(songs).collect()
}

pub fn query(terms: &[String]) -> Option<String> {
    let text = terms.join(" ").trim().to_string();
    (text.chars().count() >= SHORTEST).then_some(text)
}

pub fn kind(hit: &Hit, query: &str) -> i32 {
    if hit.name.to_lowercase() == query.to_lowercase() {
        EXACT_MATCH
    } else {
        POSSIBLE_MATCH
    }
}

pub fn page(server: &Url, path: &str) -> Option<Url> {
    let target = server.join(path).ok()?;
    (target.origin() == server.origin()).then_some(target)
}

async fn fetch(server: &Url, token: &str, text: &str) -> Result<Vec<Hit>, Failure> {
    crate::server::get_json::<Answer>(server, token, "/api/search", &[("q", text)])
        .await
        .map(hits)
}

#[derive(Clone)]
struct Search {
    app: AppHandle,
    found: Arc<Mutex<HashMap<String, Hit>>>,
}

impl Search {
    async fn find(&self, terms: &[String]) -> Vec<Hit> {
        let Some(text) = query(terms) else {
            return Vec::new();
        };
        let Some((server, token)) = crate::server::credentials(&self.app) else {
            return Vec::new();
        };
        let found = tauri::async_runtime::spawn(async move { fetch(&server, &token, &text).await })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        let mut known = self.found.lock().expect("search lock");
        for hit in &found {
            known.insert(hit.id.clone(), hit.clone());
        }
        found
    }

    fn open(&self, path: &str) {
        let Some(server) = self.app.state::<AppState>().server() else {
            return;
        };
        let Some(target) = page(&server, path) else {
            return;
        };
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || {
            let _ = crate::open_main(&app, target);
        });
    }

    fn open_hit(&self, id: &str) {
        let hit = self.found.lock().expect("search lock").get(id).cloned();
        if let Some(hit) = hit {
            self.open(&hit.url);
        }
    }

    fn open_search(&self, terms: &[String]) {
        let text = terms.join(" ");
        let mut path = Url::parse("http://x/search").expect("fixed address");
        path.query_pairs_mut().append_pair("q", text.trim());
        self.open(&format!("/search?{}", path.query().unwrap_or_default()));
    }
}

fn text(value: &str) -> OwnedValue {
    OwnedValue::try_from(Value::from(value.to_string())).expect("strings always convert")
}

struct Gnome(Search);

#[interface(name = "org.gnome.Shell.SearchProvider2")]
impl Gnome {
    async fn get_initial_result_set(&self, terms: Vec<String>) -> Vec<String> {
        self.0
            .find(&terms)
            .await
            .into_iter()
            .map(|hit| hit.id)
            .collect()
    }

    async fn get_subsearch_result_set(
        &self,
        _previous: Vec<String>,
        terms: Vec<String>,
    ) -> Vec<String> {
        self.0
            .find(&terms)
            .await
            .into_iter()
            .map(|hit| hit.id)
            .collect()
    }

    fn get_result_metas(&self, ids: Vec<String>) -> Vec<HashMap<String, OwnedValue>> {
        let known = self.0.found.lock().expect("search lock");
        ids.iter()
            .filter_map(|id| known.get(id))
            .map(|hit| {
                HashMap::from([
                    ("id".to_string(), text(&hit.id)),
                    ("name".to_string(), text(&hit.name)),
                    ("description".to_string(), text(&hit.description)),
                ])
            })
            .collect()
    }

    fn activate_result(&self, id: String, _terms: Vec<String>, _timestamp: u32) {
        self.0.open_hit(&id);
    }

    fn launch_search(&self, terms: Vec<String>, _timestamp: u32) {
        self.0.open_search(&terms);
    }
}

struct Krunner(Search);

type Match = (
    String,
    String,
    String,
    i32,
    f64,
    HashMap<String, OwnedValue>,
);

#[interface(name = "org.kde.krunner1")]
impl Krunner {
    fn actions(&self) -> Vec<(String, String, String)> {
        Vec::new()
    }

    async fn r#match(&self, query: String) -> Vec<Match> {
        let found = self.0.find(std::slice::from_ref(&query)).await;
        let count = found.len().max(1) as f64;
        found
            .into_iter()
            .enumerate()
            .map(|(index, hit)| {
                let kind = kind(&hit, query.trim());
                (
                    hit.id,
                    hit.name,
                    ICON.to_string(),
                    kind,
                    1.0 - index as f64 / (count * 2.0),
                    HashMap::from([("subtext".to_string(), text(&hit.description))]),
                )
            })
            .collect()
    }

    fn run(&self, id: String, _action: String) {
        self.0.open_hit(&id);
    }

    fn teardown(&self) {}
}

async fn serve(app: AppHandle) -> zbus::Result<Connection> {
    let search = Search {
        app,
        found: Arc::new(Mutex::new(HashMap::new())),
    };
    let connection = Connection::session().await?;
    connection
        .object_server()
        .at(PATH, Gnome(search.clone()))
        .await?;
    connection.object_server().at(PATH, Krunner(search)).await?;
    connection.request_name(BUS_NAME).await?;
    Ok(connection)
}

pub async fn run(app: AppHandle) {
    match serve(app).await {
        Ok(_connection) => std::future::pending::<()>().await,
        Err(error) => eprintln!("Couldn't offer search to the desktop: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> Answer {
        serde_json::from_str(r#"{"artists":[{"id":"radiohead","name":"Radiohead","plays":120,"url":"/artist/radiohead"}],"songs":[{"id":"radiohead-reckoner","title":"Reckoner","artist":"Radiohead","plays":1,"url":"/track/radiohead-reckoner"}]}"#).unwrap()
    }

    #[test]
    fn lists_artists_then_songs_with_their_plays() {
        let found = hits(answer());
        assert_eq!(found[0].id, "artist:radiohead");
        assert_eq!(found[0].description, "Artist · 120 plays");
        assert_eq!(found[1].id, "song:radiohead-reckoner");
        assert_eq!(found[1].name, "Reckoner");
        assert_eq!(found[1].description, "Radiohead · 1 play");
    }

    #[test]
    fn needs_two_characters_before_asking_the_server() {
        assert_eq!(query(&["r".into()]), None);
        assert_eq!(query(&[" ".into(), "r".into()]), None);
        assert_eq!(
            query(&["weird".into(), "fishes".into()]),
            Some("weird fishes".into())
        );
    }

    #[test]
    fn marks_exact_names_for_krunner() {
        let found = hits(answer());
        assert_eq!(kind(&found[0], "radiohead"), EXACT_MATCH);
        assert_eq!(kind(&found[1], "reck"), POSSIBLE_MATCH);
    }

    #[test]
    fn opens_only_pages_on_your_own_server() {
        let server = Url::parse("https://music.example.com/").unwrap();
        assert_eq!(
            page(&server, "/artist/radiohead").unwrap().as_str(),
            "https://music.example.com/artist/radiohead"
        );
        assert_eq!(page(&server, "https://evil.example.org/"), None);
        assert_eq!(page(&server, "//evil.example.org/"), None);
    }
}
