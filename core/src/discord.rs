use crate::host::{AppHandle, Manager};
use crate::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::{sleep, timeout};

const EVERY: Duration = Duration::from_secs(3);
const RETRY_AFTER: Duration = Duration::from_secs(30);
const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const LONGEST_TEXT: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Song {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    pub song: Song,
    pub position: Option<f64>,
    pub length: Option<f64>,
    pub art: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub song: Song,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub image: String,
    pub album: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Looked {
    pub image: String,
    pub album: Option<String>,
}

const NOT_ALBUMS: [&str; 4] = [
    "Soundscape",
    "30-second preview",
    "Song preview",
    "Unknown album",
];
pub const UNKNOWN_ALBUM: &str = "Unknown album";

pub fn real_album(album: Option<&str>) -> Option<&str> {
    album
        .map(str::trim)
        .filter(|album| !album.is_empty() && !NOT_ALBUMS.contains(album))
}

pub fn album_label(page: Option<&str>, looked: Option<&str>) -> String {
    real_album(page)
        .or_else(|| real_album(looked))
        .unwrap_or(UNKNOWN_ALBUM)
        .to_string()
}

pub const LOGO: &str =
    "https://raw.githubusercontent.com/novakr12/soundscape-desktop/main/icons/icon.png";
const DRIFT_MS: u64 = 2000;

fn is_private_v4(v4: Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_documentation()
        || (a == 100 && (64..=127).contains(&b))
}

pub fn is_private_host(host: &str) -> bool {
    let host = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".ts.net")
        || host.ends_with(".local")
        || host.ends_with(".lan")
        || host.ends_with(".home")
        || host.ends_with(".home.arpa")
        || host.ends_with(".internal")
        || host.ends_with(".localdomain")
        || (!host.contains('.') && !host.contains(':'))
    {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => is_private_v4(v4),
        Ok(IpAddr::V6(v6)) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6.to_ipv4_mapped().is_some_and(is_private_v4)
        }
        Err(_) => false,
    }
}

pub fn public_art(art: Option<&str>, server_host: Option<&str>) -> Option<String> {
    let art = art?;
    let url = url::Url::parse(art).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && Some(host) != server_host && !is_private_host(host))
        .then(|| art.to_string())
}

pub fn usable_cover_link(url: Option<&str>) -> Option<String> {
    public_art(url, None)
}

pub fn timing(now_ms: u64, position: Option<f64>, length: Option<f64>) -> (u64, Option<u64>) {
    let start = now_ms.saturating_sub((position.unwrap_or(0.0).max(0.0) * 1000.0) as u64);
    let end = length
        .filter(|length| *length > 0.0)
        .map(|length| start + (length * 1000.0) as u64);
    (start, end)
}

pub fn needs_update(before: Option<&Shown>, after: Option<&Shown>) -> bool {
    match (before, after) {
        (None, None) => false,
        (Some(before), Some(after)) => {
            before.song != after.song
                || before.image != after.image
                || before.album != after.album
                || before.end_ms.is_some() != after.end_ms.is_some()
                || before.start_ms.abs_diff(after.start_ms) > DRIFT_MS
        }
        _ => true,
    }
}

pub fn valid_app_id(text: &str) -> bool {
    (17..=20).contains(&text.len()) && text.chars().all(|c| c.is_ascii_digit())
}

pub fn socket_paths(runtime: Option<&str>, temp: Option<&str>) -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(runtime) = runtime {
        let runtime = PathBuf::from(runtime);
        bases.push(runtime.join("app/com.discordapp.Discord"));
        bases.push(runtime.join("snap.discord"));
        bases.push(runtime.clone());
    }
    bases.push(PathBuf::from(temp.unwrap_or("/tmp")));
    bases
        .iter()
        .flat_map(|base| (0..10).map(move |i| base.join(format!("discord-ipc-{i}"))))
        .collect()
}

pub fn frame(op: u32, body: &Value) -> Vec<u8> {
    let text = body.to_string();
    let mut out = Vec::with_capacity(8 + text.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

fn short(text: &str) -> String {
    let text = text.trim();
    let mut out: String = text.chars().take(LONGEST_TEXT).collect();
    while out.chars().count() < 2 {
        out.push(' ');
    }
    out
}

pub fn activity(shown: Option<&Shown>, pid: u32, nonce: u64) -> Value {
    let activity = shown.map(|shown| {
        let mut timestamps = json!({ "start": shown.start_ms });
        if let Some(end) = shown.end_ms {
            timestamps["end"] = json!(end);
        }
        json!({
            "type": 2,
            "status_display_type": 0,
            "details": short(&shown.song.title),
            "state": short(&format!("by {}", shown.song.artist)),
            "timestamps": timestamps,
            "assets": { "large_image": shown.image, "large_text": short(&shown.album) },
        })
    });
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": pid, "activity": activity },
        "nonce": nonce.to_string(),
    })
}

fn current(app: &AppHandle) -> Option<Playing> {
    let state = app.state::<AppState>();
    if let Some(source) = state.mini().filter(|source| source.playing) {
        return Some(Playing {
            song: Song {
                title: source.track.title,
                artist: source.track.artist,
                album: source.track.album,
            },
            position: source.position_ms.map(|ms| ms as f64 / 1000.0),
            length: source.track.length_ms.map(|ms| ms as f64 / 1000.0),
            art: source.art,
        });
    }
    state
        .preview()
        .filter(|preview| preview.playing)
        .map(|preview| Playing {
            song: Song {
                title: preview.title,
                artist: preview.artist,
                album: preview.album,
            },
            position: Some(preview.position),
            length: preview.length,
            art: preview.cover,
        })
}

#[derive(serde::Deserialize)]
struct CoverLink {
    url: Option<String>,
    #[serde(default)]
    album: Option<String>,
}

#[derive(Clone, Debug)]
struct Cached {
    looked: Looked,
    retry_at: Option<std::time::Instant>,
}

const COVER_RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_FRAME: usize = 1 << 20;

async fn look_up(app: &AppHandle, playing: &Playing, known: &mut HashMap<Song, Cached>) -> Looked {
    let now = std::time::Instant::now();
    let mut previous_album = None;
    if let Some(cached) = known.get(&playing.song) {
        match cached.retry_at {
            None => return cached.looked.clone(),
            Some(at) if now < at => return cached.looked.clone(),
            Some(_) => previous_album = cached.looked.album.clone(),
        }
    }
    let credentials = crate::server::credentials(app);
    let server_host = credentials
        .as_ref()
        .and_then(|(server, _)| server.host_str().map(str::to_string));
    let art = public_art(playing.art.as_deref(), server_host.as_deref());
    let page_album = real_album(playing.song.album.as_deref()).map(str::to_string);
    let needs_lookup = art.is_none() || page_album.is_none();
    let mut lookup_ok = !needs_lookup;
    let mut asked: Option<CoverLink> = None;
    if needs_lookup {
        if let Some((server, token)) = credentials {
            let song = &playing.song;
            if let Ok(link) = crate::server::get_json::<CoverLink>(
                &server,
                &token,
                "/api/cover-link",
                &[
                    ("artist", song.artist.as_str()),
                    ("title", song.title.as_str()),
                ],
            )
            .await
            {
                asked = Some(link);
                lookup_ok = true;
            }
        }
    }
    let (url, album) = asked.map_or((None, None), |link| {
        (usable_cover_link(link.url.as_deref()), link.album)
    });
    let resolved_image = art.or(url);
    let resolved_album = page_album.or(album).or(previous_album);
    let looked = Looked {
        image: resolved_image.clone().unwrap_or_else(|| LOGO.to_string()),
        album: resolved_album.clone(),
    };
    let complete = resolved_image.is_some() && (resolved_album.is_some() || lookup_ok);
    let retry_at = (!complete).then(|| std::time::Instant::now() + COVER_RETRY_AFTER);
    if known.len() > 200 {
        known.clear();
    }
    known.insert(
        playing.song.clone(),
        Cached {
            looked: looked.clone(),
            retry_at,
        },
    );
    looked
}

async fn read_frame<R: tokio::io::AsyncRead + Unpin>(stream: &mut R) -> std::io::Result<()> {
    let mut header = [0u8; 8];
    timeout(Duration::from_secs(5), stream.read_exact(&mut header))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Discord frame timeout")
        })??;
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if length > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Discord frame too large",
        ));
    }
    let mut body = vec![0u8; length];
    timeout(Duration::from_secs(5), stream.read_exact(&mut body))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Discord body timeout"))??;
    Ok(())
}

async fn connect(app_id: &str) -> Option<UnixStream> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok();
    let temp = std::env::var("TMPDIR").ok();
    for path in socket_paths(runtime.as_deref(), temp.as_deref()) {
        let Ok(mut stream) = UnixStream::connect(&path).await else {
            continue;
        };
        let hello = frame(HANDSHAKE, &json!({ "v": 1, "client_id": app_id }));
        let wrote = timeout(Duration::from_secs(5), stream.write_all(&hello)).await;
        if matches!(wrote, Ok(Ok(()))) && read_frame(&mut stream).await.is_ok() {
            return Some(stream);
        }
    }
    None
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn wanted(app: &AppHandle) -> Option<String> {
    let config = app.state::<AppState>().config();
    config
        .discord_app_id
        .filter(|id| config.discord && valid_app_id(id))
}

pub async fn run(app: AppHandle) {
    let mut stream: Option<(String, UnixStream)> = None;
    let mut shown: Option<Option<Shown>> = None;
    let mut looked: HashMap<Song, Cached> = HashMap::new();
    let mut nonce = 0u64;
    loop {
        let Some(app_id) = wanted(&app) else {
            stream = None;
            shown = None;
            if !looked.is_empty() {
                looked.clear();
                looked.shrink_to_fit();
            }
            sleep(EVERY).await;
            continue;
        };
        if stream.as_ref().is_some_and(|(id, _)| *id != app_id) {
            stream = None;
        }
        if stream.is_none() {
            match connect(&app_id).await {
                Some(connected) => {
                    stream = Some((app_id.clone(), connected));
                    shown = None;
                }
                None => {
                    sleep(RETRY_AFTER).await;
                    continue;
                }
            }
        }
        let next = match current(&app) {
            Some(playing) => {
                let (start_ms, end_ms) = timing(now_ms(), playing.position, playing.length);
                let found = look_up(&app, &playing, &mut looked).await;
                Some(Shown {
                    album: album_label(playing.song.album.as_deref(), found.album.as_deref()),
                    image: found.image,
                    song: playing.song,
                    start_ms,
                    end_ms,
                })
            }
            None => None,
        };
        let changed = match &shown {
            None => true,
            Some(before) => needs_update(before.as_ref(), next.as_ref()),
        };
        if changed {
            nonce += 1;
            let message = frame(FRAME, &activity(next.as_ref(), std::process::id(), nonce));
            let sent = match stream.as_mut() {
                Some((_, socket)) => {
                    let wrote = timeout(Duration::from_secs(5), socket.write_all(&message)).await;
                    matches!(wrote, Ok(Ok(()))) && read_frame(socket).await.is_ok()
                }
                None => false,
            };
            if sent {
                shown = Some(next);
            } else {
                stream = None;
                continue;
            }
        }
        sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_application_id() {
        assert!(valid_app_id("1234567890123456789"));
        assert!(!valid_app_id("12345"));
        assert!(!valid_app_id("12345678901234567x9"));
        assert!(!valid_app_id(""));
    }

    #[test]
    fn looks_for_discord_in_the_usual_places() {
        let paths = socket_paths(Some("/run/user/1000"), None);
        assert_eq!(
            paths[0],
            PathBuf::from("/run/user/1000/app/com.discordapp.Discord/discord-ipc-0")
        );
        assert!(paths.contains(&PathBuf::from("/run/user/1000/discord-ipc-0")));
        assert!(paths.contains(&PathBuf::from("/run/user/1000/snap.discord/discord-ipc-3")));
        assert_eq!(paths.last(), Some(&PathBuf::from("/tmp/discord-ipc-9")));
        assert_eq!(paths.len(), 40);
    }

    #[test]
    fn frames_messages_the_way_discord_reads_them() {
        let bytes = frame(1, &json!({ "a": 1 }));
        assert_eq!(&bytes[0..4], &1u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &7u32.to_le_bytes());
        assert_eq!(&bytes[8..], br#"{"a":1}"#);
        assert!(
            crate::host::async_runtime::block_on(read_frame(&mut std::io::Cursor::new(
                bytes.as_slice()
            )))
            .is_ok()
        );
        let mut huge = Vec::new();
        huge.extend_from_slice(&1u32.to_le_bytes());
        huge.extend_from_slice(&((MAX_FRAME as u32) + 1).to_le_bytes());
        let err = crate::host::async_runtime::block_on(read_frame(&mut std::io::Cursor::new(
            huge.as_slice(),
        )))
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    fn song() -> Song {
        Song {
            title: "Need It".into(),
            artist: "Migos".into(),
            album: Some("Culture III".into()),
        }
    }

    #[test]
    fn describes_the_song_like_a_music_app() {
        let shown = Shown {
            song: song(),
            start_ms: 1_700_000_000_000,
            end_ms: Some(1_700_000_200_000),
            image: "https://cdn.example/cover.jpg".into(),
            album: "Culture III".into(),
        };
        let message = activity(Some(&shown), 42, 7);
        assert_eq!(message["cmd"], "SET_ACTIVITY");
        assert_eq!(message["nonce"], "7");
        assert_eq!(message["args"]["pid"], 42);
        let sent = &message["args"]["activity"];
        assert_eq!(sent["type"], 2);
        assert_eq!(sent["details"], "Need It");
        assert_eq!(sent["state"], "by Migos");
        assert_eq!(sent["timestamps"]["start"], 1_700_000_000_000u64);
        assert_eq!(sent["timestamps"]["end"], 1_700_000_200_000u64);
        assert_eq!(
            sent["assets"]["large_image"],
            "https://cdn.example/cover.jpg"
        );
        assert_eq!(sent["assets"]["large_text"], "Culture III");
        assert!(activity(None, 42, 8)["args"]["activity"].is_null());
        let open_ended = Shown {
            end_ms: None,
            ..shown
        };
        assert!(
            activity(Some(&open_ended), 1, 1)["args"]["activity"]["timestamps"]
                .get("end")
                .is_none()
        );
    }

    #[test]
    fn always_names_an_album() {
        assert_eq!(album_label(Some("Culture III"), None), "Culture III");
        assert_eq!(
            album_label(Some("Soundscape"), Some("Culture III")),
            "Culture III"
        );
        assert_eq!(album_label(None, Some("Culture III")), "Culture III");
        assert_eq!(
            album_label(Some("30-second preview"), None),
            "Unknown album"
        );
        assert_eq!(album_label(Some("  "), Some("")), "Unknown album");
        assert_eq!(album_label(None, None), "Unknown album");
    }

    #[test]
    fn works_out_where_the_song_started_and_ends() {
        assert_eq!(
            timing(100_000, Some(30.0), Some(200.0)),
            (70_000, Some(270_000))
        );
        assert_eq!(timing(100_000, None, None), (100_000, None));
        assert_eq!(timing(10_000, Some(30.0), Some(0.0)), (0, None));
    }

    #[test]
    fn only_uses_covers_anyone_can_see() {
        assert_eq!(
            public_art(Some("https://i.scdn.co/image/abc"), Some("music.example")).as_deref(),
            Some("https://i.scdn.co/image/abc")
        );
        assert_eq!(
            public_art(
                Some("https://is1-ssl.mzstatic.com/image/thumb/Music/cover.jpg/600x600bb.jpg"),
                Some("box.tail1234.ts.net")
            )
            .as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music/cover.jpg/600x600bb.jpg")
        );
        assert_eq!(
            public_art(
                Some("https://music.example/api/navidrome/cover/1"),
                Some("music.example")
            ),
            None
        );
        assert_eq!(
            public_art(Some("file:///home/me/cover.jpg"), Some("music.example")),
            None
        );
        assert_eq!(public_art(Some("http://cdn.example/a.jpg"), None), None);
        assert_eq!(public_art(None, None), None);

        for private in [
            "box.tail1234.ts.net",
            "100.64.0.1",
            "100.127.255.254",
            "192.168.1.5",
            "10.0.0.1",
            "172.16.0.1",
            "localhost",
            "nas.local",
            "nas",
            "[::1]",
            "[fd7a:115c:a1e0::1]",
            "[::ffff:100.64.0.1]",
        ] {
            assert!(is_private_host(private), "{private} should be private");
            let url = format!("https://{private}/api/cover/a-b");
            assert_eq!(usable_cover_link(Some(&url)), None);
        }
        assert!(!is_private_host("100.63.255.254"));
        assert!(!is_private_host("100.128.0.1"));
        assert_eq!(
            usable_cover_link(Some("https://music.example/api/cover/a-b")).as_deref(),
            Some("https://music.example/api/cover/a-b")
        );
    }

    #[test]
    fn sends_again_only_when_something_shows_differently() {
        let first = Shown {
            song: song(),
            start_ms: 100_000,
            end_ms: Some(300_000),
            image: LOGO.into(),
            album: UNKNOWN_ALBUM.into(),
        };
        let a_moment_later = Shown {
            start_ms: 100_900,
            end_ms: Some(300_900),
            ..first.clone()
        };
        let after_a_seek = Shown {
            start_ms: 60_000,
            end_ms: Some(260_000),
            ..first.clone()
        };
        assert!(!needs_update(Some(&first), Some(&a_moment_later)));
        assert!(needs_update(Some(&first), Some(&after_a_seek)));
        assert!(needs_update(Some(&first), None));
        assert!(needs_update(None, Some(&first)));
        assert!(!needs_update(None, None));
        let other_song = Shown {
            song: Song {
                title: "Bad and Boujee".into(),
                ..song()
            },
            ..first.clone()
        };
        assert!(needs_update(Some(&first), Some(&other_song)));
        let album_found = Shown {
            album: "Culture III".into(),
            ..first.clone()
        };
        assert!(needs_update(Some(&first), Some(&album_found)));
    }
}
