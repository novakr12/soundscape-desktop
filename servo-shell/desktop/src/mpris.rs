use crate::scrobble::{Snapshot, Track};
use std::collections::HashMap;
use zbus::proxy::{Builder, CacheProperties};
use zbus::zvariant::OwnedValue;
use zbus::{Connection, Proxy};

pub const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const ROOT: &str = "org.mpris.MediaPlayer2";
const BROWSERS: &[&str] = &[
    "firefox",
    "librewolf",
    "zen",
    "chromium",
    "chrome",
    "google-chrome",
    "brave",
    "vivaldi",
    "opera",
    "msedge",
    "microsoft-edge",
    "epiphany",
    "plasma-browser-integration",
    "kdeconnect",
];
const IGNORED: &[&str] = &["playerctld", "soundscapedesktop"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub id: String,
    pub name: String,
    pub bus: String,
    pub art: Option<String>,
    pub snapshot: Snapshot,
}

pub async fn control(bus: &str, method: &str) -> zbus::Result<()> {
    let connection = Connection::session().await?;
    let player = proxy(&connection, bus, PLAYER).await?;
    player.call_method(method, &()).await?;
    Ok(())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub shuffle: Option<bool>,
    pub repeat: Option<&'static str>,
    pub can_seek: bool,
}

pub fn repeat_name(loop_status: &str) -> Option<&'static str> {
    match loop_status {
        "None" => Some("off"),
        "Playlist" => Some("all"),
        "Track" => Some("one"),
        _ => None,
    }
}

pub fn next_loop(loop_status: &str) -> &'static str {
    match loop_status {
        "None" => "Playlist",
        "Playlist" => "Track",
        _ => "None",
    }
}

pub async fn modes(bus: &str) -> Modes {
    let Ok(connection) = Connection::session().await else {
        return Modes::default();
    };
    let Ok(player) = proxy(&connection, bus, PLAYER).await else {
        return Modes::default();
    };
    let loop_status: Option<String> = player.get_property("LoopStatus").await.ok();
    Modes {
        shuffle: player.get_property("Shuffle").await.ok(),
        repeat: loop_status.as_deref().and_then(repeat_name),
        can_seek: player.get_property("CanSeek").await.unwrap_or(false),
    }
}

pub async fn toggle_shuffle(bus: &str) -> zbus::Result<()> {
    let connection = Connection::session().await?;
    let player = proxy(&connection, bus, PLAYER).await?;
    let shuffle: bool = player.get_property("Shuffle").await?;
    player.set_property("Shuffle", !shuffle).await?;
    Ok(())
}

pub async fn cycle_repeat(bus: &str) -> zbus::Result<()> {
    let connection = Connection::session().await?;
    let player = proxy(&connection, bus, PLAYER).await?;
    let loop_status: String = player.get_property("LoopStatus").await?;
    player
        .set_property("LoopStatus", next_loop(&loop_status))
        .await?;
    Ok(())
}

pub async fn seek_to(bus: &str, position_ms: u64) -> zbus::Result<()> {
    let connection = Connection::session().await?;
    let player = proxy(&connection, bus, PLAYER).await?;
    let target = i64::try_from(position_ms.saturating_mul(1000)).unwrap_or(i64::MAX);
    let metadata: HashMap<String, OwnedValue> =
        player.get_property("Metadata").await.unwrap_or_default();
    let track = metadata
        .get("mpris:trackid")
        .and_then(|value| zbus::zvariant::OwnedObjectPath::try_from(value.clone()).ok());
    if let Some(track) = track {
        if player
            .call_method("SetPosition", &(track, target))
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    let now: i64 = player.get_property("Position").await?;
    player.call_method("Seek", &(target - now)).await?;
    Ok(())
}

pub fn player_id(bus_name: &str) -> Option<String> {
    let rest = bus_name.strip_prefix(PREFIX)?;
    let id = match rest.find(".instance") {
        Some(index) => &rest[..index],
        None => rest,
    };
    let id = id.to_lowercase();
    if id.is_empty() || IGNORED.contains(&id.as_str()) {
        None
    } else {
        Some(id)
    }
}

pub fn on_by_default(id: &str) -> bool {
    !BROWSERS
        .iter()
        .any(|browser| id == *browser || id.starts_with(&format!("{browser}.")))
}

pub fn is_browser(id: &str) -> bool {
    !on_by_default(id) && id != "kdeconnect" && !id.starts_with("kdeconnect.")
}

fn text(value: &OwnedValue) -> Option<String> {
    String::try_from(value.clone())
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn texts(value: &OwnedValue) -> Vec<String> {
    if let Ok(list) = Vec::<String>::try_from(value.clone()) {
        return list
            .into_iter()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .collect();
    }
    text(value).into_iter().collect()
}

fn micros(value: &OwnedValue) -> Option<u64> {
    if let Ok(number) = i64::try_from(value.clone()) {
        return u64::try_from(number).ok();
    }
    if let Ok(number) = u64::try_from(value.clone()) {
        return Some(number);
    }
    f64::try_from(value.clone())
        .ok()
        .filter(|number| *number >= 0.0)
        .map(|number| number as u64)
}

fn file_name(value: &OwnedValue) -> Option<String> {
    let url = url::Url::parse(&text(value)?).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    Some(path.file_name()?.to_string_lossy().into_owned())
}

pub fn track_from(metadata: &HashMap<String, OwnedValue>) -> Option<Track> {
    let title = metadata
        .get("xesam:title")
        .and_then(text)
        .or_else(|| metadata.get("xesam:url").and_then(file_name))?;
    let mut artists = metadata.get("xesam:artist").map(texts).unwrap_or_default();
    if artists.is_empty() {
        artists = metadata
            .get("xesam:albumArtist")
            .map(texts)
            .unwrap_or_default();
    }
    Some(Track {
        artist: artists.join(", "),
        title,
        album: metadata.get("xesam:album").and_then(text),
        length_ms: metadata
            .get("mpris:length")
            .and_then(micros)
            .filter(|length| *length > 0)
            .map(|length| length / 1000),
    })
}

async fn proxy<'a>(
    connection: &Connection,
    name: &'a str,
    interface: &'a str,
) -> zbus::Result<Proxy<'a>> {
    Builder::<Proxy>::new(connection)
        .destination(name)?
        .path(PATH)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

async fn read(connection: &Connection, name: &str, id: String) -> zbus::Result<Player> {
    let player = proxy(connection, name, PLAYER).await?;
    let status: String = player.get_property("PlaybackStatus").await?;
    let metadata: HashMap<String, OwnedValue> =
        player.get_property("Metadata").await.unwrap_or_default();
    let position = player
        .get_property::<i64>("Position")
        .await
        .ok()
        .and_then(|p| u64::try_from(p).ok())
        .map(|p| p / 1000);
    let root = proxy(connection, name, ROOT).await?;
    let identity: String = root
        .get_property("Identity")
        .await
        .unwrap_or_else(|_| id.clone());
    Ok(Player {
        bus: name.to_string(),
        art: metadata.get("mpris:artUrl").and_then(text),
        name: if identity.trim().is_empty() {
            id.clone()
        } else {
            identity.trim().to_string()
        },
        snapshot: Snapshot {
            player: id.clone(),
            playing: status == "Playing",
            track: track_from(&metadata).and_then(|track| {
                if is_browser(&id) {
                    crate::tidy::from_browser(track)
                } else {
                    crate::tidy::from_file(track)
                }
            }),
            position_ms: position,
        },
        id,
    })
}

pub fn parent_pid(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn is_this_app(pid: u32) -> bool {
    let me = std::process::id();
    if pid == me {
        return true;
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parent_pid(&stat))
        == Some(me)
}

async fn from_this_app(bus: &zbus::fdo::DBusProxy<'_>, name: &str) -> bool {
    let Ok(name) = zbus::names::BusName::try_from(name) else {
        return false;
    };
    bus.get_connection_unix_process_id(name)
        .await
        .is_ok_and(is_this_app)
}

pub async fn players(connection: &Connection) -> zbus::Result<Vec<Player>> {
    list(connection, true).await
}

async fn list(connection: &Connection, skip_this_app: bool) -> zbus::Result<Vec<Player>> {
    let bus = zbus::fdo::DBusProxy::new(connection).await?;
    let names = bus.list_names().await?;
    let mut found: Vec<Player> = Vec::new();
    for name in names {
        let Some(id) = player_id(name.as_str()) else {
            continue;
        };
        if found.iter().any(|player| player.id == id)
            || (skip_this_app && from_this_app(&bus, name.as_str()).await)
        {
            continue;
        }
        if let Ok(player) = read(connection, name.as_str(), id).await {
            found.push(player);
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_loop_status_into_repeat_and_cycles_it_like_soundscape() {
        assert_eq!(repeat_name("None"), Some("off"));
        assert_eq!(repeat_name("Playlist"), Some("all"));
        assert_eq!(repeat_name("Track"), Some("one"));
        assert_eq!(repeat_name("Weird"), None);
        assert_eq!(next_loop("None"), "Playlist");
        assert_eq!(next_loop("Playlist"), "Track");
        assert_eq!(next_loop("Track"), "None");
    }
    use zbus::interface;
    use zbus::zvariant::{Str, Value};

    struct FakeRoot;

    #[interface(name = "org.mpris.MediaPlayer2")]
    impl FakeRoot {
        #[zbus(property)]
        fn identity(&self) -> String {
            "Fake Player".into()
        }
    }

    struct FakePlayer;

    #[interface(name = "org.mpris.MediaPlayer2.Player")]
    impl FakePlayer {
        #[zbus(property)]
        fn playback_status(&self) -> String {
            "Playing".into()
        }

        #[zbus(property)]
        fn metadata(&self) -> HashMap<String, OwnedValue> {
            let mut map = HashMap::new();
            map.insert(
                "xesam:title".into(),
                OwnedValue::try_from(Value::from("Weird Fishes")).unwrap(),
            );
            map.insert(
                "xesam:artist".into(),
                OwnedValue::try_from(Value::from(vec!["Radiohead"])).unwrap(),
            );
            map.insert("mpris:length".into(), OwnedValue::from(318_000_000i64));
            map
        }

        #[zbus(property)]
        fn position(&self) -> i64 {
            42_000_000
        }
    }

    #[test]
    fn reads_a_running_player_from_the_session_bus() {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            eprintln!("No session bus, skipping");
            return;
        }
        crate::host::async_runtime::block_on(async {
            let name = format!(
                "org.mpris.MediaPlayer2.soundscapetest.instance{}",
                std::process::id()
            );
            let _served = zbus::connection::Builder::session()
                .unwrap()
                .name(name)
                .unwrap()
                .serve_at(PATH, FakeRoot)
                .unwrap()
                .serve_at(PATH, FakePlayer)
                .unwrap()
                .build()
                .await
                .unwrap();
            let reader = Connection::session().await.unwrap();
            assert!(players(&reader)
                .await
                .unwrap()
                .iter()
                .all(|p| p.id != "soundscapetest"));
            let found = list(&reader, false).await.unwrap();
            let player = found
                .iter()
                .find(|player| player.id == "soundscapetest")
                .expect("the test player");
            assert_eq!(player.name, "Fake Player");
            assert!(player.snapshot.playing);
            assert_eq!(player.snapshot.position_ms, Some(42_000));
            assert_eq!(
                player.snapshot.track,
                Some(Track {
                    artist: "Radiohead".into(),
                    title: "Weird Fishes".into(),
                    album: None,
                    length_ms: Some(318_000)
                })
            );
        });
    }

    fn owned(value: Value<'_>) -> OwnedValue {
        value.try_to_owned().unwrap()
    }

    #[test]
    fn finds_the_parent_process() {
        assert_eq!(
            parent_pid("7278 (WebKitWebProces) S 7211 7210 7210 0 -1"),
            Some(7211)
        );
        assert_eq!(parent_pid("12 (odd ) name) R 34 12 12"), Some(34));
        assert_eq!(parent_pid("garbage"), None);
        assert!(is_this_app(std::process::id()));
    }

    #[test]
    fn names_players_by_their_bus_name() {
        assert_eq!(
            player_id("org.mpris.MediaPlayer2.spotify"),
            Some("spotify".into())
        );
        assert_eq!(
            player_id("org.mpris.MediaPlayer2.firefox.instance_1_42"),
            Some("firefox".into())
        );
        assert_eq!(
            player_id("org.mpris.MediaPlayer2.chromium.instance12345"),
            Some("chromium".into())
        );
        assert_eq!(player_id("org.mpris.MediaPlayer2.playerctld"), None);
        assert_eq!(player_id("org.freedesktop.Notifications"), None);
    }

    #[test]
    fn leaves_browsers_off_until_you_turn_them_on() {
        assert!(on_by_default("spotify"));
        assert!(on_by_default("strawberry"));
        assert!(!on_by_default("firefox"));
        assert!(!on_by_default("chromium"));
        assert!(!on_by_default("plasma-browser-integration"));
        assert!(is_browser("firefox"));
        assert!(!is_browser("kdeconnect"));
        assert!(!is_browser("strawberry"));
        assert_eq!(player_id(crate::media::BUS_NAME), None);
    }

    #[test]
    fn reads_song_details_the_way_players_send_them() {
        let mut metadata = HashMap::new();
        metadata.insert(
            "xesam:title".to_string(),
            owned(Value::from("Karma Police")),
        );
        metadata.insert(
            "xesam:artist".to_string(),
            owned(Value::from(vec!["Radiohead"])),
        );
        metadata.insert("xesam:album".to_string(), owned(Value::from("OK Computer")));
        metadata.insert(
            "mpris:length".to_string(),
            owned(Value::from(264_000_000i64)),
        );
        assert_eq!(
            track_from(&metadata),
            Some(Track {
                artist: "Radiohead".into(),
                title: "Karma Police".into(),
                album: Some("OK Computer".into()),
                length_ms: Some(264_000)
            })
        );

        metadata.insert(
            "xesam:artist".to_string(),
            owned(Value::from(vec!["Daft Punk", "Pharrell Williams"])),
        );
        metadata.insert(
            "mpris:length".to_string(),
            owned(Value::from(248_000_000u64)),
        );
        let track = track_from(&metadata).unwrap();
        assert_eq!(track.artist, "Daft Punk, Pharrell Williams");
        assert_eq!(track.length_ms, Some(248_000));

        metadata.insert(
            "xesam:artist".to_string(),
            owned(Value::Str(Str::from("Single String Artist"))),
        );
        assert_eq!(
            track_from(&metadata).unwrap().artist,
            "Single String Artist"
        );

        let mut untagged = HashMap::new();
        untagged.insert(
            "xesam:url".to_string(),
            owned(Value::from(
                "file:///home/me/Music/03%20-%20Air%20-%20Talisman.flac",
            )),
        );
        let track = track_from(&untagged).unwrap();
        assert_eq!(
            (track.artist.as_str(), track.title.as_str()),
            ("", "03 - Air - Talisman.flac")
        );
        untagged.insert(
            "xesam:url".to_string(),
            owned(Value::from("https://radio.example/stream")),
        );
        assert_eq!(track_from(&untagged), None);

        metadata.remove("xesam:artist");
        assert_eq!(track_from(&metadata).unwrap().artist, "");
        metadata.insert(
            "xesam:albumArtist".to_string(),
            owned(Value::from(vec!["Various"])),
        );
        assert_eq!(track_from(&metadata).unwrap().artist, "Various");
        metadata.remove("xesam:title");
        assert_eq!(track_from(&metadata), None);
    }
}
