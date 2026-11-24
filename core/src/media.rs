use crate::host::{AppHandle, Manager};
use crate::MAIN;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

pub const BUS_NAME: &str = "org.mpris.MediaPlayer2.SoundscapeDesktop";
const PATH: &str = "/org/mpris/MediaPlayer2";
const TRACK: &str = "/io/github/novakr12/SoundscapeDesktop/Preview";
const EVERY: Duration = Duration::from_millis(1000);
const IDLE: Duration = Duration::from_secs(4);

static POKED: Notify = Notify::const_new();

pub fn poke() {
    POKED.notify_one();
}

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
    length: Number.isFinite(audio.duration) && audio.duration > 0 ? audio.duration : (() => {
      try {
        const queue = JSON.parse(localStorage.getItem("soundscape:queue") ?? "null");
        const track = queue?.tracks?.[queue.order?.[queue.position]];
        return typeof track?.duration === "number" && track.duration > 0 ? track.duration : null;
      } catch {
        return null;
      }
    })(),
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

pub fn next_wait(preview: Option<&Preview>) -> Duration {
    if preview.is_some() {
        EVERY
    } else {
        IDLE
    }
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

struct Root(AppHandle);

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {
        crate::show(&self.0);
    }
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
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
        if crate::native_link::control(&self.app, action, seconds) {
            self.shared.wake.notify_one();
            return;
        }
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

pub async fn evaluate(app: &AppHandle, script: impl Into<String>) -> Option<String> {
    crate::page::evaluate(app, script.into()).await
}

async fn read(app: &AppHandle) -> Option<Preview> {
    if let Some(preview) = crate::native_link::preview(app) {
        return Some(preview);
    }
    parse(&evaluate(app, READ).await?)
}

fn mpris_preview(
    preview: Option<Preview>,
    credentials: Option<(url::Url, String)>,
    relay: &mut Option<(url::Url, String, crate::relay::Relay)>,
) -> Option<Preview> {
    let Some((server, token)) = credentials else {
        *relay = None;
        return preview;
    };
    if relay
        .as_ref()
        .is_some_and(|(kept_server, kept_token, _)| *kept_server != server || *kept_token != token)
    {
        *relay = None;
    }
    let mut preview = preview?;
    let proxied = preview.cover.as_deref().and_then(|cover| {
        crate::relay::cover_route(&server, cover)?;
        if relay.is_none() {
            *relay = crate::relay::Relay::start(server.clone(), token.clone())
                .ok()
                .map(|started| (server.clone(), token.clone(), started));
        }
        relay
            .as_ref()
            .and_then(|(_, _, active)| active.cover_url(&server, cover))
    });
    if let Some(proxied) = proxied {
        preview.cover = Some(proxied);
    }
    Some(preview)
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
    let connection = crate::bus::session().await.ok()?;
    let server = connection.object_server();
    let _ = server.at(PATH, Root(app.clone())).await;
    let _ = server
        .at(
            PATH,
            Player {
                app: app.clone(),
                shared: shared.clone(),
            },
        )
        .await;
    Some(connection)
}

pub fn control(app: &AppHandle, action: &str) {
    control_at(app, action, 0.0);
}

pub fn control_at(app: &AppHandle, action: &str, seconds: f64) {
    if crate::native_link::control(app, action, seconds) {
        return;
    }
    if let (Some(window), Some(js)) = (app.get_webview_window(MAIN), script(action, seconds)) {
        let _ = window.eval(js);
    }
}

pub async fn run(app: AppHandle) {
    let shared = Arc::new(Shared::default());
    let mut connection: Option<Connection> = None;
    let mut named = false;
    let mut cover_relay: Option<(url::Url, String, crate::relay::Relay)> = None;
    let mut wait = IDLE;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shared.wake.notified() => {}
            _ = POKED.notified() => {}
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        let preview = read(&app).await;
        wait = next_wait(preview.as_ref());
        app.state::<crate::AppState>().set_preview(preview.clone());
        let mpris = mpris_preview(preview, crate::server::credentials(&app), &mut cover_relay);
        let old = {
            let mut current = shared.preview.lock().expect("preview lock");
            std::mem::replace(&mut *current, mpris.clone())
        };
        if mpris.is_some() && connection.is_none() {
            connection = connect(&app, &shared).await;
        }
        let Some(bus) = &connection else { continue };
        if mpris.is_some() && !named {
            named = bus.request_name(BUS_NAME).await.is_ok();
        }
        announce(bus, old.as_ref(), mpris.as_ref()).await;
        if mpris.is_none() && named {
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
    fn looks_often_only_while_something_is_loaded() {
        let preview = parse(
            r#"{"title":"Says","artist":"Nils Frahm","album":null,"cover":null,"playing":false,"position":3,"length":60}"#,
        );
        assert_eq!(next_wait(preview.as_ref()), EVERY);
        assert_eq!(next_wait(None), IDLE);
    }

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

    #[test]
    fn proxies_only_same_server_covers_for_mpris() {
        let server = url::Url::parse("https://box.tail1234.ts.net/").unwrap();
        let creds = Some((server.clone(), "tok".into()));
        let mut relay = None;
        let base = Preview {
            title: "Dopesmoker".into(),
            artist: "Sleep".into(),
            album: Some("Dopesmoker".into()),
            cover: Some("https://box.tail1234.ts.net/api/navidrome/cover/al-1".into()),
            playing: true,
            position: 10.0,
            length: Some(3800.0),
            has_next: false,
            has_previous: false,
            shuffle: None,
            repeat: None,
            up_next: None,
        };
        let proxied = mpris_preview(Some(base.clone()), creds.clone(), &mut relay).unwrap();
        let cover = proxied.cover.as_deref().unwrap();
        assert!(cover.starts_with("http://127.0.0.1:"));
        assert!(cover.ends_with("/navidrome-cover/al-1"));
        assert!(relay.is_some());

        let track_cover = Preview {
            cover: Some("https://box.tail1234.ts.net/api/cover/sleep-dopesmoker".into()),
            ..base.clone()
        };
        let proxied_track = mpris_preview(Some(track_cover), creds.clone(), &mut relay).unwrap();
        assert_eq!(
            proxied_track.cover.as_deref().map(|s| s.rsplit('/').next()),
            Some(Some("sleep-dopesmoker"))
        );

        let external = Preview {
            cover: Some("https://cdn-images.dzcdn.net/images/cover/abc/300x300.jpg".into()),
            ..base.clone()
        };
        let unchanged = mpris_preview(Some(external.clone()), creds, &mut relay).unwrap();
        assert_eq!(unchanged.cover, external.cover);

        let cleared = mpris_preview(Some(base), None, &mut relay).unwrap();
        assert!(relay.is_none());
        assert!(cleared
            .cover
            .unwrap()
            .starts_with("https://box.tail1234.ts.net/"));
    }
}
