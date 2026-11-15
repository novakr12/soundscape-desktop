use crate::AppState;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const EVERY: Duration = Duration::from_secs(5);
const RETRY_AFTER: Duration = Duration::from_secs(30);
const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const LONGEST_TEXT: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playing {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
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

pub fn activity(playing: Option<&Playing>, started: u64, pid: u32, nonce: u64) -> Value {
    let activity = playing.map(|song| {
        let mut assets = json!({});
        if let Some(album) = song
            .album
            .as_deref()
            .filter(|album| !album.trim().is_empty())
        {
            assets = json!({ "large_text": short(album) });
        }
        json!({
            "type": 2,
            "details": short(&song.title),
            "state": short(&format!("by {}", song.artist)),
            "timestamps": { "start": started },
            "assets": assets,
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
            title: source.track.title,
            artist: source.track.artist,
            album: source.track.album,
        });
    }
    state
        .preview()
        .filter(|preview| preview.playing)
        .map(|preview| Playing {
            title: preview.title,
            artist: preview.artist,
            album: preview.album,
        })
}

fn read_frame(stream: &mut UnixStream) -> std::io::Result<()> {
    let mut header = [0u8; 8];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let mut body = vec![0u8; length.min(1 << 20)];
    stream.read_exact(&mut body)
}

fn connect(app_id: &str) -> Option<UnixStream> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok();
    let temp = std::env::var("TMPDIR").ok();
    for path in socket_paths(runtime.as_deref(), temp.as_deref()) {
        let Ok(mut stream) = UnixStream::connect(&path) else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let hello = frame(HANDSHAKE, &json!({ "v": 1, "client_id": app_id }));
        if stream.write_all(&hello).is_ok() && read_frame(&mut stream).is_ok() {
            return Some(stream);
        }
    }
    None
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn wanted(app: &AppHandle) -> Option<String> {
    let config = app.state::<AppState>().config();
    config
        .discord_app_id
        .filter(|id| config.discord && valid_app_id(id))
}

pub fn run(app: AppHandle) {
    std::thread::spawn(move || {
        let mut stream: Option<(String, UnixStream)> = None;
        let mut shown: Option<Option<Playing>> = None;
        let mut started;
        let mut nonce = 0u64;
        loop {
            let Some(app_id) = wanted(&app) else {
                stream = None;
                shown = None;
                std::thread::sleep(EVERY);
                continue;
            };
            if stream.as_ref().is_some_and(|(id, _)| *id != app_id) {
                stream = None;
            }
            if stream.is_none() {
                match connect(&app_id) {
                    Some(connected) => {
                        stream = Some((app_id.clone(), connected));
                        shown = None;
                    }
                    None => {
                        std::thread::sleep(RETRY_AFTER);
                        continue;
                    }
                }
            }
            let playing = current(&app);
            if shown.as_ref() != Some(&playing) {
                started = now_seconds();
                nonce += 1;
                let message = frame(
                    FRAME,
                    &activity(playing.as_ref(), started, std::process::id(), nonce),
                );
                let sent = stream.as_mut().is_some_and(|(_, socket)| {
                    socket.write_all(&message).is_ok() && read_frame(socket).is_ok()
                });
                if sent {
                    shown = Some(playing);
                } else {
                    stream = None;
                    continue;
                }
            }
            std::thread::sleep(EVERY);
        }
    });
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
    }

    #[test]
    fn describes_the_song_as_listening() {
        let song = Playing {
            title: "Reckoner".into(),
            artist: "Radiohead".into(),
            album: Some("In Rainbows".into()),
        };
        let message = activity(Some(&song), 1_700_000_000, 42, 7);
        assert_eq!(message["cmd"], "SET_ACTIVITY");
        assert_eq!(message["nonce"], "7");
        assert_eq!(message["args"]["pid"], 42);
        let shown = &message["args"]["activity"];
        assert_eq!(shown["type"], 2);
        assert_eq!(shown["details"], "Reckoner");
        assert_eq!(shown["state"], "by Radiohead");
        assert_eq!(shown["timestamps"]["start"], 1_700_000_000u64);
        assert_eq!(shown["assets"]["large_text"], "In Rainbows");
        let cleared = activity(None, 0, 42, 8);
        assert!(cleared["args"]["activity"].is_null());
        let tiny = Playing {
            title: "X".into(),
            artist: "Y".into(),
            album: None,
        };
        assert_eq!(
            activity(Some(&tiny), 0, 1, 1)["args"]["activity"]["details"],
            "X "
        );
    }
}
