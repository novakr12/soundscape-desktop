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
const IGNORED: &[&str] = &["playerctld"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    pub id: String,
    pub name: String,
    pub snapshot: Snapshot,
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

pub fn track_from(metadata: &HashMap<String, OwnedValue>) -> Option<Track> {
    let title = metadata.get("xesam:title").and_then(text)?;
    let mut artists = metadata.get("xesam:artist").map(texts).unwrap_or_default();
    if artists.is_empty() {
        artists = metadata
            .get("xesam:albumArtist")
            .map(texts)
            .unwrap_or_default();
    }
    if artists.is_empty() {
        return None;
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
                    Some(track)
                }
            }),
            position_ms: position,
        },
        id,
    })
}

pub async fn players(connection: &Connection) -> zbus::Result<Vec<Player>> {
    let names = zbus::fdo::DBusProxy::new(connection)
        .await?
        .list_names()
        .await?;
    let mut found: Vec<Player> = Vec::new();
    for name in names {
        let Some(id) = player_id(name.as_str()) else {
            continue;
        };
        if found.iter().any(|player| player.id == id) {
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
        tauri::async_runtime::block_on(async {
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
            let found = players(&reader).await.unwrap();
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

        metadata.remove("xesam:artist");
        assert_eq!(track_from(&metadata), None);
        metadata.insert(
            "xesam:albumArtist".to_string(),
            owned(Value::from(vec!["Various"])),
        );
        assert_eq!(track_from(&metadata).unwrap().artist, "Various");
        metadata.remove("xesam:title");
        assert_eq!(track_from(&metadata), None);
    }
}
