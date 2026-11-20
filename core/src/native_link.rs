use crate::host::{async_runtime, AppHandle, Manager};
use crate::media::Preview;
use crate::native::{self, Command, Handover, Player, Snapshot, Track};
use crate::{AppState, MAIN};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;
use url::Url;

pub const ASKED: &str = "soundscape-native ";
pub const KIND: &str = "desktop";
pub const DEFAULT_VOLUME: f64 = 0.8;
const EVERY: Duration = Duration::from_secs(1);
const REPORT_AFTER: Duration = Duration::from_millis(250);
const MOST_TRACKS: usize = 500;
const MOST_UP_NEXT: usize = 10;
const REPEAT_MODES: [&str; 3] = ["off", "all", "one"];

static VOLUME: Mutex<f64> = Mutex::new(DEFAULT_VOLUME);

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "do", rename_all = "lowercase")]
pub enum Asked {
    Hello,
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Seek {
        time: f64,
    },
    Volume {
        volume: f64,
    },
    Take {
        tracks: Vec<Track>,
        position: usize,
        #[serde(default)]
        time: f64,
        #[serde(default = "yes")]
        playing: bool,
    },
    Send {
        to: String,
    },
    Enqueue {
        track: Track,
        #[serde(default)]
        next: bool,
    },
    Shuffle {
        on: bool,
    },
    Repeat {
        mode: String,
    },
    Jump {
        ahead: usize,
    },
}

pub fn asked(message: &str) -> Option<Asked> {
    serde_json::from_str(message.strip_prefix(ASKED)?).ok()
}

fn with_player<T>(app: &AppHandle, read: impl FnOnce(&Player) -> T) -> Option<T> {
    let state = app.state::<AppState>();
    let player = state.native.lock().expect("native lock");
    player.as_ref().map(read)
}

pub fn active(app: &AppHandle) -> bool {
    with_player(app, |_| ()).is_some()
}

pub fn preview(app: &AppHandle) -> Option<Preview> {
    with_player(app, Player::preview).flatten()
}

pub fn control(app: &AppHandle, action: &str, seconds: f64) -> bool {
    let Some(command) = native::command_for(action, seconds) else {
        return active(app) && matches!(action, "shuffle");
    };
    let sent = with_player(app, |player| player.send(command)).is_some();
    if sent {
        report_soon(app);
    }
    sent
}

pub fn wanted(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let config = state.config();
    config.native_player && config.connected() && !state.signed_out()
}

pub fn is_device_id(text: &str) -> bool {
    (8..=64).contains(&text.len())
        && text
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || letter == '_' || letter == '-')
}

pub fn new_device_id(seed: u128) -> String {
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut left = seed;
    (0..20)
        .map(|_| {
            let letter = LETTERS[(left % LETTERS.len() as u128) as usize] as char;
            left /= LETTERS.len() as u128;
            letter
        })
        .collect()
}

fn seed() -> u128 {
    use std::hash::{BuildHasher, Hasher};
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    let mut first = std::collections::hash_map::RandomState::new().build_hasher();
    first.write_u128(time);
    first.write_u32(std::process::id());
    let mut second = std::collections::hash_map::RandomState::new().build_hasher();
    second.write_u64(first.finish());
    (u128::from(first.finish()) << 64) | u128::from(second.finish())
}

pub fn device_id(app: &AppHandle) -> String {
    let state = app.state::<AppState>();
    if let Some(id) = state.config().device_id.filter(|id| is_device_id(id)) {
        return id;
    }
    let id = new_device_id(seed());
    state.update(|config| config.device_id = Some(id.clone()));
    id
}

pub fn device_name() -> String {
    crate::server::device_name()
}

pub fn page_script(id: &str, name: &str) -> String {
    format!(
        r#"(() => {{
  if (window.top !== window) return;
  window.soundscapeDeviceId = {};
  window.soundscapeDeviceName = {};
  window.soundscapeNative = {{ postMessage: (message) => console.log({} + String(message)) }};
}})();"#,
        Value::from(id),
        Value::from(name),
        Value::from(ASKED)
    )
}

pub fn script_for(app: &AppHandle) -> Option<String> {
    wanted(app).then(|| page_script(&device_id(app), &device_name()))
}

fn track_json(track: &Track) -> Value {
    json!({
        "trackId": track.track_id,
        "title": track.title,
        "artist": track.artist,
        "album": track.album,
        "cover": track.cover,
        "songId": track.song_id,
    })
}

pub fn report_json(id: &str, name: &str, snapshot: Option<&Snapshot>, volume: f64) -> String {
    let state = match snapshot {
        Some(now) => json!({
            "track": track_json(&now.track),
            "playing": now.playing,
            "time": if now.time.is_finite() && now.time > 0.0 { now.time } else { 0.0 },
            "duration": now.duration.filter(|length| length.is_finite() && *length > 0.0),
            "volume": now.volume.clamp(0.0, 1.0),
            "blocked": false,
            "repeat": REPEAT_MODES.contains(&now.repeat.as_str()).then_some(now.repeat.as_str()),
            "upNext": now
                .up_next
                .iter()
                .take(MOST_UP_NEXT)
                .map(|next| json!({ "trackId": next.track_id, "title": next.title, "artist": next.artist }))
                .collect::<Vec<_>>(),
        }),
        None => json!({
            "track": null,
            "playing": false,
            "time": 0,
            "duration": null,
            "volume": volume.clamp(0.0, 1.0),
            "blocked": false,
            "repeat": null,
            "upNext": [],
        }),
    };
    json!({ "id": id, "name": name, "kind": KIND, "state": state }).to_string()
}

pub fn report_script(json: &str) -> String {
    format!(
        "(() => {{ const report = window.soundscapeNativeReport; if (report) report({json}); }})()"
    )
}

fn volume() -> f64 {
    *VOLUME.lock().expect("volume lock")
}

pub fn report(app: &AppHandle) {
    if !wanted(app) {
        return;
    }
    let snapshot = with_player(app, Player::snapshot).flatten();
    let json = report_json(&device_id(app), &device_name(), snapshot.as_ref(), volume());
    app.host().eval(MAIN, report_script(&json));
}

pub fn report_soon(app: &AppHandle) {
    let app = app.clone();
    async_runtime::spawn(async move {
        tokio::time::sleep(REPORT_AFTER).await;
        report(&app);
    });
}

fn scrobbles(server: Url, token: String) -> native::Report {
    Box::new(move |kind, track| {
        let server = server.clone();
        let token = token.clone();
        async_runtime::spawn(async move {
            let path = if kind == "play" {
                "/api/play"
            } else {
                "/api/now-playing"
            };
            let Ok(address) = crate::server::address(&server, path) else {
                return;
            };
            let mut body = json!({ "trackId": track.track_id });
            if let Some(song) = &track.song_id {
                body["songId"] = Value::from(song.clone());
            }
            let _ =
                crate::server::answer(crate::server::client().post(address).json(&body), &token)
                    .await;
        });
    })
}

pub fn queue_window(tracks: Vec<Track>, position: usize) -> Option<(Vec<Track>, usize)> {
    if tracks.is_empty() || position >= tracks.len() {
        return None;
    }
    let from = position
        .saturating_sub(MOST_TRACKS / 2)
        .min(tracks.len().saturating_sub(MOST_TRACKS));
    let part: Vec<Track> = tracks.into_iter().skip(from).take(MOST_TRACKS).collect();
    Some((part, position - from))
}

fn start(app: &AppHandle, tracks: Vec<Track>, position: usize, time: f64, playing: bool) {
    let Some((tracks, position)) = queue_window(tracks, position) else {
        return;
    };
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    let repeat = with_player(app, Player::snapshot)
        .flatten()
        .map_or_else(|| "off".to_string(), |now| now.repeat);
    let handover = Handover {
        tracks,
        position,
        time: time.max(0.0),
        playing,
        volume: volume(),
        muted: false,
        levelling: true,
        repeat,
    };
    let state = app.state::<AppState>();
    let mut current = state.native.lock().expect("native lock");
    current.take();
    match native::start(
        server.clone(),
        token.clone(),
        handover,
        scrobbles(server, token),
        false,
    ) {
        Ok(player) => *current = Some(player),
        Err(error) => eprintln!("The built-in player couldn't start: {error}"),
    }
}

fn send_elsewhere(app: &AppHandle, to: String) {
    let Some(now) = with_player(app, Player::snapshot).flatten() else {
        return;
    };
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    let mut tracks = vec![now.track.clone()];
    tracks.extend(now.up_next.iter().cloned());
    let body = json!({
        "to": to,
        "command": {
            "do": "take",
            "tracks": tracks.iter().map(|track| {
                let mut item = track_json(track);
                if let Some(length) = track.duration {
                    item["duration"] = Value::from(length);
                }
                item
            }).collect::<Vec<_>>(),
            "position": 0,
            "time": now.time.max(0.0),
            "playing": true,
        },
    });
    let app = app.clone();
    async_runtime::spawn(async move {
        let Ok(address) = crate::server::address(&server, "/api/devices/command") else {
            return;
        };
        let sent = crate::server::answer(crate::server::client().post(address).json(&body), &token)
            .await
            .is_ok();
        if sent {
            with_player(&app, |player| player.send(Command::Pause));
            report_soon(&app);
        }
    });
}

pub fn from_page(app: &AppHandle, asked: Asked) {
    if !wanted(app) {
        return;
    }
    let send = |command: Command| {
        with_player(app, |player| player.send(command));
    };
    match asked {
        Asked::Hello => {}
        Asked::Play => send(Command::Play),
        Asked::Pause => send(Command::Pause),
        Asked::Toggle => send(Command::Toggle),
        Asked::Next => send(Command::Next),
        Asked::Previous => send(Command::Previous),
        Asked::Seek { time } if time.is_finite() && time >= 0.0 => send(Command::SeekTo(time)),
        Asked::Volume { volume } if (0.0..=1.0).contains(&volume) => {
            *VOLUME.lock().expect("volume lock") = volume;
            send(Command::Volume(volume));
        }
        Asked::Take {
            tracks,
            position,
            time,
            playing,
        } => start(app, tracks, position, time, playing),
        Asked::Send { to } if is_device_id(&to) => send_elsewhere(app, to),
        Asked::Enqueue { track, next } => {
            if active(app) {
                send(Command::Enqueue(Box::new(track), next));
            } else {
                start(app, vec![track], 0, 0.0, true);
            }
        }
        Asked::Repeat { mode } if REPEAT_MODES.contains(&mode.as_str()) => {
            send(Command::SetRepeat(mode))
        }
        Asked::Jump { ahead } if (1..=MOST_UP_NEXT).contains(&ahead) => send(Command::Jump(ahead)),
        _ => {}
    }
    report(app);
    report_soon(app);
}

pub async fn run(app: AppHandle) {
    loop {
        tokio::time::sleep(EVERY).await;
        if active(&app) {
            report(&app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            track_id: id.into(),
            song_id: Some(format!("nd-{id}")),
            title: format!("Song {id}"),
            artist: "Band".into(),
            album: Some("Album".into()),
            cover: Some("/api/navidrome/cover/al-1".into()),
            gain: None,
            duration: Some(200.0),
        }
    }

    #[test]
    fn reads_what_the_page_asks_for() {
        assert_eq!(
            asked(r#"soundscape-native {"do":"hello"}"#),
            Some(Asked::Hello)
        );
        assert_eq!(
            asked(r#"soundscape-native {"do":"seek","time":42.5}"#),
            Some(Asked::Seek { time: 42.5 })
        );
        let take = asked(
            r#"soundscape-native {"do":"take","tracks":[{"trackId":"a","title":"A","artist":"B","albumHref":"/album/x","duration":180}],"position":0,"time":3,"playing":true}"#,
        );
        let Some(Asked::Take {
            tracks,
            position,
            time,
            playing,
        }) = take
        else {
            panic!("take");
        };
        assert_eq!((tracks.len(), position, time, playing), (1, 0, 3.0, true));
        assert_eq!(tracks[0].duration, Some(180.0));
        assert_eq!(
            asked(r#"soundscape-native {"do":"enqueue","track":{"trackId":"a","title":"A","artist":"B"},"next":true}"#)
                .map(|asked| matches!(asked, Asked::Enqueue { next: true, .. })),
            Some(true)
        );
        assert_eq!(asked(r#"soundscape-native {"do":"dance"}"#), None);
        assert_eq!(asked(r#"something else {"do":"play"}"#), None);
    }

    #[test]
    fn reports_like_the_phone_does() {
        let now = Snapshot {
            track: track("a"),
            playing: true,
            time: 12.0,
            duration: Some(200.0),
            volume: 0.6,
            repeat: "all".into(),
            up_next: vec![track("b")],
        };
        let report: Value =
            serde_json::from_str(&report_json("abcdefgh12", "Desktop app", Some(&now), 0.8))
                .unwrap();
        assert_eq!(report["kind"], "desktop");
        assert_eq!(report["state"]["track"]["trackId"], "a");
        assert_eq!(report["state"]["track"]["songId"], "nd-a");
        assert_eq!(report["state"]["upNext"][0]["title"], "Song b");
        assert_eq!(report["state"]["repeat"], "all");
        let idle: Value =
            serde_json::from_str(&report_json("abcdefgh12", "Desktop app", None, 0.5)).unwrap();
        assert_eq!(idle["state"]["track"], Value::Null);
        assert_eq!(idle["state"]["volume"], 0.5);
        assert!(report_script("{}").contains("soundscapeNativeReport"));
    }

    #[test]
    fn makes_ids_the_server_accepts() {
        let id = new_device_id(123_456_789_012_345_678_901_234_567_890);
        assert_eq!(id.len(), 20);
        assert!(is_device_id(&id));
        assert!(!is_device_id("short"));
        assert!(!is_device_id("has spaces in it"));
        assert_ne!(new_device_id(1), new_device_id(2));
    }

    #[test]
    fn gives_the_page_a_way_to_reach_the_player() {
        let script = page_script("abcdefgh12", "Desktop app on \"box\"");
        assert!(script.contains(r#"window.soundscapeDeviceId = "abcdefgh12";"#));
        assert!(script.contains(r#"Desktop app on \"box\""#));
        assert!(script.contains("soundscape-native "));
    }

    #[test]
    fn keeps_a_long_queue_around_the_song() {
        let tracks: Vec<Track> = (0..800).map(|index| track(&index.to_string())).collect();
        let (part, at) = queue_window(tracks, 700).unwrap();
        assert_eq!(part.len(), 500);
        assert_eq!(part[at].track_id, "700");
        assert_eq!(queue_window(Vec::new(), 0), None);
        assert_eq!(queue_window(vec![track("a")], 1), None);
    }
}
