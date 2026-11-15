use crate::MAIN;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::sync::{oneshot, Notify};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.SoundscapeDesktop";
const PATH: &str = "/org/mpris/MediaPlayer2";
const TRACK: &str = "/io/github/novakr12/SoundscapeDesktop/Preview";
const EVERY: Duration = Duration::from_millis(1000);

const READ: &str = r#"(() => {
  const audio = document.querySelector(".player audio") ?? document.querySelector("audio");
  const meta = navigator.mediaSession?.metadata;
  if (!audio || !meta || !audio.currentSrc) return "null";
  return JSON.stringify({
    title: meta.title,
    artist: meta.artist,
    album: meta.album || null,
    cover: meta.artwork?.[0]?.src ?? null,
    playing: !audio.paused && !audio.ended,
    position: Number.isFinite(audio.currentTime) ? audio.currentTime : 0,
    length: Number.isFinite(audio.duration) ? audio.duration : null,
    hasNext: Boolean(document.querySelector('[data-player="next"]:not([disabled])')),
    hasPrevious: Boolean(document.querySelector('[data-player="previous"]')),
    shuffle: (() => {
      const button = document.querySelector('[data-player="shuffle"]');
      return button ? button.getAttribute("aria-pressed") === "true" : null;
    })(),
    repeat: document.querySelector('[data-player="repeat"]')?.dataset.repeat ?? null,
    upNext: (() => {
      try {
        const queue = JSON.parse(localStorage.getItem("soundscape:queue") ?? "null");
        const repeat = document.querySelector('[data-player="repeat"]')?.dataset.repeat;
        if (!queue || !Array.isArray(queue.order) || !Array.isArray(queue.tracks) || repeat === "one") return null;
        let at = queue.position + 1;
        if (at >= queue.order.length) at = repeat === "all" && queue.order.length > 1 ? 0 : -1;
        const track = at >= 0 ? queue.tracks[queue.order[at]] : null;
        return track && typeof track.title === "string" && typeof track.artist === "string"
          ? { title: track.title, artist: track.artist }
          : null;
      } catch {
        return null;
      }
    })(),
  });
})()"#;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct UpNext {
    pub title: String,
    pub artist: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Preview {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub cover: Option<String>,
    pub playing: bool,
    pub position: f64,
    pub length: Option<f64>,
    #[serde(default, rename = "hasNext")]
    pub has_next: bool,
    #[serde(default, rename = "hasPrevious")]
    pub has_previous: bool,
    #[serde(default)]
    pub shuffle: Option<bool>,
    #[serde(default)]
    pub repeat: Option<String>,
    #[serde(default, rename = "upNext")]
    pub up_next: Option<UpNext>,
}

impl Preview {
    fn same_song(&self, other: &Preview) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.cover == other.cover
            && self.length.map(|l| l.round()) == other.length.map(|l| l.round())
    }
}

pub fn parse(json: &str) -> Option<Preview> {
    serde_json::from_str::<Option<Preview>>(json)
        .ok()
        .flatten()
        .filter(|preview| !preview.title.trim().is_empty())
}

fn micros(seconds: f64) -> i64 {
    (seconds.max(0.0) * 1_000_000.0) as i64
}

fn value(value: Value<'_>) -> OwnedValue {
    OwnedValue::try_from(value).expect("plain values always convert")
}

pub fn metadata(preview: Option<&Preview>) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let Some(preview) = preview else {
        map.insert(
            "mpris:trackid".into(),
            value(Value::from(ObjectPath::from_static_str_unchecked(
                "/org/mpris/MediaPlayer2/TrackList/NoTrack",
            ))),
        );
        return map;
    };
    map.insert(
        "mpris:trackid".into(),
        value(Value::from(ObjectPath::from_static_str_unchecked(TRACK))),
    );
    map.insert(
        "xesam:title".into(),
        value(Value::from(preview.title.clone())),
    );
    map.insert(
        "xesam:artist".into(),
        value(Value::from(vec![preview.artist.clone()])),
    );
    if let Some(album) = &preview.album {
        map.insert("xesam:album".into(), value(Value::from(album.clone())));
    }
    if let Some(cover) = &preview.cover {
        map.insert("mpris:artUrl".into(), value(Value::from(cover.clone())));
    }
    if let Some(length) = preview.length {
        map.insert("mpris:length".into(), value(Value::from(micros(length))));
    }
    map
}

pub fn script(action: &str, seconds: f64) -> Option<String> {
    let body = match action {
        "play" => "a.play()".to_string(),
        "pause" => "a.pause()".to_string(),
        "toggle" => "a.paused || a.ended ? a.play() : a.pause()".to_string(),
        "stop" => "a.pause(); a.currentTime = 0".to_string(),
        "seek" => format!("a.currentTime = Math.max(0, a.currentTime + {seconds})"),
        "position" => format!("a.currentTime = {}", seconds.max(0.0)),
        "next" | "previous" | "shuffle" | "repeat" => {
            return Some(format!(
                "document.querySelector('[data-player=\"{action}\"]:not([disabled])')?.click()"
            ))
        }
        _ => return None,
    };
    Some(format!(
        "(() => {{ const a = document.querySelector(\".player audio\") ?? document.querySelector(\"audio\"); if (a) {{ {body}; }} }})()"
    ))
}

#[derive(Default)]
struct Shared {
    preview: Mutex<Option<Preview>>,
    wake: Notify,
}

impl Shared {
    fn preview(&self) -> Option<Preview> {
        self.preview.lock().expect("preview lock").clone()
    }
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        "Soundscape".into()
    }
    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "io.github.novakr12.SoundscapeDesktop".into()
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    app: AppHandle,
    shared: Arc<Shared>,
}

impl Player {
    fn run(&self, action: &str, seconds: f64) {
        if let (Some(window), Some(js)) =
            (self.app.get_webview_window(MAIN), script(action, seconds))
        {
            let _ = window.eval(js);
        }
        self.shared.wake.notify_one();
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play(&self) {
        self.run("play", 0.0);
    }
    fn pause(&self) {
        self.run("pause", 0.0);
    }
    fn play_pause(&self) {
        self.run("toggle", 0.0);
    }
    fn stop(&self) {
        self.run("stop", 0.0);
    }
    fn next(&self) {
        self.run("next", 0.0);
    }
    fn previous(&self) {
        self.run("previous", 0.0);
    }
    fn seek(&self, offset: i64) {
        self.run("seek", offset as f64 / 1_000_000.0);
    }
    fn set_position(&self, track: ObjectPath<'_>, position: i64) {
        if track.as_str() == TRACK {
            self.run("position", position as f64 / 1_000_000.0);
        }
    }
    fn open_uri(&self, _uri: String) {}
    #[zbus(property)]
    fn playback_status(&self) -> String {
        match self.shared.preview() {
            Some(preview) if preview.playing => "Playing",
            Some(_) => "Paused",
            None => "Stopped",
        }
        .into()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(self.shared.preview().as_ref())
    }
    #[zbus(property)]
    fn position(&self) -> i64 {
        self.shared
            .preview()
            .map_or(0, |preview| micros(preview.position))
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.shared
            .preview()
            .is_some_and(|preview| preview.has_next)
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.shared
            .preview()
            .is_some_and(|preview| preview.has_previous)
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.shared.preview().is_some()
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.shared.preview().is_some()
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.shared.preview().is_some()
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

async fn read(app: &AppHandle) -> Option<Preview> {
    use javascriptcore::ValueExt;
    use webkit2gtk::WebViewExt;
    let window = app.get_webview_window(MAIN)?;
    let (send, receive) = oneshot::channel::<Option<String>>();
    window
        .with_webview(move |webview| {
            webview.inner().evaluate_javascript(
                READ,
                None,
                None,
                None::<&webkit2gtk::gio::Cancellable>,
                move |result| {
                    let _ = send.send(result.ok().map(|value| value.to_str().to_string()));
                },
            );
        })
        .ok()?;
    let json = tokio::time::timeout(Duration::from_secs(2), receive)
        .await
        .ok()?
        .ok()??;
    parse(&json)
}

async fn announce(connection: &Connection, old: Option<&Preview>, new: Option<&Preview>) {
    let Ok(player) = connection
        .object_server()
        .interface::<_, Player>(PATH)
        .await
    else {
        return;
    };
    let emitter: &SignalEmitter<'_> = player.signal_emitter();
    let player = player.get().await;
    let song_changed = match (old, new) {
        (Some(old), Some(new)) => !old.same_song(new),
        (None, None) => false,
        _ => true,
    };
    if song_changed
        || old.map(|p| (p.has_next, p.has_previous)) != new.map(|p| (p.has_next, p.has_previous))
    {
        let _ = player.can_go_next_changed(emitter).await;
        let _ = player.can_go_previous_changed(emitter).await;
    }
    if song_changed {
        let _ = player.metadata_changed(emitter).await;
        let _ = player.can_play_changed(emitter).await;
        let _ = player.can_pause_changed(emitter).await;
        let _ = player.can_seek_changed(emitter).await;
    }
    if old.map(|p| p.playing) != new.map(|p| p.playing) || old.is_some() != new.is_some() {
        let _ = player.playback_status_changed(emitter).await;
    }
}

async fn connect(app: &AppHandle, shared: &Arc<Shared>) -> Option<Connection> {
    let connection = Connection::session().await.ok()?;
    let server = connection.object_server();
    server.at(PATH, Root).await.ok()?;
    server
        .at(
            PATH,
            Player {
                app: app.clone(),
                shared: shared.clone(),
            },
        )
        .await
        .ok()?;
    Some(connection)
}

pub fn control(app: &AppHandle, action: &str) {
    control_at(app, action, 0.0);
}

pub fn control_at(app: &AppHandle, action: &str, seconds: f64) {
    if let (Some(window), Some(js)) = (app.get_webview_window(MAIN), script(action, seconds)) {
        let _ = window.eval(js);
    }
}

pub async fn run(app: AppHandle) {
    let shared = Arc::new(Shared::default());
    let mut connection: Option<Connection> = None;
    let mut named = false;
    loop {
        let _ = tokio::time::timeout(EVERY, shared.wake.notified()).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let mut preview = read(&app).await;
        app.state::<crate::AppState>().set_preview(preview.clone());
        if let (Some(_), Some(bus)) = (&preview, &connection) {
            if crate::mpris::webkit_shows_this_app(bus, BUS_NAME).await {
                preview = None;
            }
        }
        let old = {
            let mut current = shared.preview.lock().expect("preview lock");
            std::mem::replace(&mut *current, preview.clone())
        };
        if preview.is_some() && connection.is_none() {
            connection = connect(&app, &shared).await;
        }
        let Some(bus) = &connection else { continue };
        if preview.is_some() && !named {
            named = bus.request_name(BUS_NAME).await.is_ok();
        }
        announce(bus, old.as_ref(), preview.as_ref()).await;
        if preview.is_none() && named {
            named = bus
                .release_name(BUS_NAME)
                .await
                .map(|_| false)
                .unwrap_or(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_page_reports() {
        let preview = parse(r#"{"title":"Says","artist":"Nils Frahm","album":"30-second preview","cover":null,"playing":true,"position":3.2,"length":29.9}"#).unwrap();
        assert_eq!(preview.title, "Says");
        assert!(preview.playing);
        assert_eq!(parse("null"), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(
            parse(
                r#"{"title":" ","artist":"x","album":null,"cover":null,"playing":true,"position":0,"length":null}"#
            ),
            None
        );
    }

    #[test]
    fn reads_shuffle_repeat_and_the_next_song() {
        let preview = parse(r#"{"title":"Says","artist":"Nils Frahm","album":null,"cover":null,"playing":true,"position":3,"length":60,"shuffle":true,"repeat":"all","upNext":{"title":"Teardrop","artist":"Massive Attack"}}"#).unwrap();
        assert_eq!(preview.shuffle, Some(true));
        assert_eq!(preview.repeat.as_deref(), Some("all"));
        assert_eq!(preview.up_next.unwrap().title, "Teardrop");
        let older = parse(r#"{"title":"Says","artist":"Nils Frahm","album":null,"cover":null,"playing":true,"position":3,"length":60}"#).unwrap();
        assert_eq!(
            (older.shuffle, older.repeat, older.up_next),
            (None, None, None)
        );
    }

    #[test]
    fn describes_the_preview_for_media_keys() {
        let preview = parse(r#"{"title":"Says","artist":"Nils Frahm","album":"30-second preview","cover":"https://example.com/a.jpg","playing":true,"position":3.2,"length":29.9}"#).unwrap();
        let map = metadata(Some(&preview));
        assert_eq!(
            String::try_from(map["xesam:title"].clone()).unwrap(),
            "Says"
        );
        assert_eq!(
            i64::try_from(map["mpris:length"].clone()).unwrap(),
            29_900_000
        );
        assert!(map.contains_key("mpris:artUrl"));
        assert!(!metadata(None).contains_key("xesam:title"));
    }

    #[test]
    fn turns_media_keys_into_page_actions() {
        assert!(script("toggle", 0.0)
            .unwrap()
            .contains("a.paused || a.ended ? a.play() : a.pause()"));
        assert!(script("seek", -5.0).unwrap().contains("a.currentTime + -5"));
        assert!(script("position", -1.0)
            .unwrap()
            .contains("a.currentTime = 0"));
        assert_eq!(script("rm -rf", 0.0), None);
        assert_eq!(
            script("shuffle", 0.0).unwrap(),
            "document.querySelector('[data-player=\"shuffle\"]:not([disabled])')?.click()"
        );
        assert_eq!(
            script("next", 0.0).unwrap(),
            "document.querySelector('[data-player=\"next\"]:not([disabled])')?.click()"
        );
    }
}
