use crate::host::{async_runtime, AppHandle, Manager};
use crate::media::Preview;
use crate::native::{self, Command, Handover, Player, Snapshot, Track};
use crate::{AppState, MAIN};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use url::Url;

pub use soundscape_protocol::NATIVE_ASKED as ASKED;
pub const KIND: &str = "desktop";
pub const DEFAULT_VOLUME: f64 = 0.8;
const EVERY: Duration = Duration::from_secs(1);
const REPORT_AFTER: Duration = Duration::from_millis(250);
const MOST_TRACKS: usize = 500;
const MOST_UP_NEXT: usize = 10;
const REPEAT_MODES: [&str; 3] = ["off", "all", "one"];

const HEARTBEAT: Duration = Duration::from_secs(30);
const RETRY_AFTER: Duration = Duration::from_secs(5);
const CROSSFADE_CHOICES: [f64; 6] = [0.0, 2.0, 4.0, 6.0, 8.0, 12.0];

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct PageSettings {
    #[serde(default)]
    pub volume: Option<f64>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default = "yes")]
    pub levelling: bool,
    #[serde(default)]
    pub crossfade: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub volume: f64,
    pub muted: bool,
    pub levelling: bool,
    pub crossfade: f64,
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        volume: DEFAULT_VOLUME,
        muted: false,
        levelling: true,
        crossfade: 0.0,
    };

    pub fn with(self, page: PageSettings) -> Settings {
        Settings {
            volume: page
                .volume
                .filter(|volume| (0.0..=1.0).contains(volume))
                .unwrap_or(self.volume),
            muted: page.muted,
            levelling: page.levelling,
            crossfade: if CROSSFADE_CHOICES.contains(&page.crossfade) {
                page.crossfade
            } else {
                0.0
            },
        }
    }

    fn command(self) -> Command {
        Command::Settings {
            volume: self.volume,
            muted: self.muted,
            levelling: self.levelling,
            crossfade: self.crossfade,
        }
    }
}

static SETTINGS: Mutex<Settings> = Mutex::new(Settings::DEFAULT);
static SHUFFLE: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
struct Carried {
    settings: Option<PageSettings>,
}

pub fn carried(message: &str) -> Option<PageSettings> {
    serde_json::from_str::<Carried>(message.strip_prefix(ASKED)?)
        .ok()?
        .settings
}

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
    Autoplay {
        on: bool,
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

fn settled(app: &AppHandle) -> Option<Option<Snapshot>> {
    match with_player(app, Player::snapshot) {
        Some(None) => None,
        Some(snapshot) => Some(snapshot),
        None => Some(None),
    }
}

pub fn active(app: &AppHandle) -> bool {
    with_player(app, |_| ()).is_some()
}

pub fn preview(app: &AppHandle) -> Option<Preview> {
    with_player(app, Player::preview).flatten()
}

pub fn control(app: &AppHandle, action: &str, seconds: f64) -> bool {
    let Some(command) = native::command_for(action, seconds) else {
        return false;
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
  window.soundscapeDeviceHeld = true;
  const prefix = {};
  const settings = () => {{
    try {{
      const volume = JSON.parse(localStorage.getItem("soundscape-volume") ?? "null");
      const options = JSON.parse(localStorage.getItem("soundscape:player-options") ?? "null");
      return {{
        volume: typeof volume?.volume === "number" ? volume.volume : null,
        muted: volume?.muted === true,
        levelling: options?.levelling !== false,
        crossfade: typeof options?.crossfade === "number" ? options.crossfade : 0,
      }};
    }} catch {{
      return null;
    }}
  }};
  const post = (message) => {{
    let sent = String(message);
    try {{
      const parsed = JSON.parse(sent);
      parsed.settings = settings();
      sent = JSON.stringify(parsed);
    }} catch {{}}
    console.log(prefix + sent);
  }};
  window.soundscapeNative = {{ postMessage: post }};
  window.addEventListener("soundscape:player-options", () => post(JSON.stringify({{ do: "hello" }})));
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
            "shuffle": now.shuffle,
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
            "shuffle": null,
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

fn settings() -> Settings {
    *SETTINGS.lock().expect("settings lock")
}

fn volume() -> f64 {
    settings().volume
}

fn apply(app: &AppHandle, page: PageSettings) {
    let fresh = {
        let mut stored = SETTINGS.lock().expect("settings lock");
        let fresh = stored.with(page);
        if *stored == fresh {
            return;
        }
        *stored = fresh;
        fresh
    };
    with_player(app, |player| player.send(fresh.command()));
}

static LAST_REPORT: Mutex<Option<String>> = Mutex::new(None);
static RUN_WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
const IDLE_EVERY: Duration = Duration::from_secs(5);

fn reset_report() {
    *LAST_REPORT.lock().expect("report lock") = None;
}

pub fn report(app: &AppHandle) {
    if !wanted(app) {
        reset_report();
        return;
    }
    let Some(snapshot) = settled(app) else {
        return;
    };
    let json = report_json(&device_id(app), &device_name(), snapshot.as_ref(), volume());
    {
        let mut last = LAST_REPORT.lock().expect("report lock");
        if last.as_deref() == Some(json.as_str()) {
            return;
        }
        *last = Some(json.clone());
    }
    app.host().eval(MAIN, report_script(&json));
}

pub fn report_soon(app: &AppHandle) {
    let app = app.clone();
    async_runtime::spawn(async move {
        tokio::time::sleep(REPORT_AFTER).await;
        report(&app);
        report_to_server(&app).await;
    });
}

pub async fn report_to_server(app: &AppHandle) {
    if !wanted(app) {
        return;
    }
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    let Some(snapshot) = settled(app) else {
        return;
    };
    let json = report_json(&device_id(app), &device_name(), snapshot.as_ref(), volume());
    let Ok(body) = serde_json::from_str::<Value>(&json) else {
        return;
    };
    let Ok(address) = crate::server::address(&server, "/api/devices") else {
        return;
    };
    let _ = crate::server::answer(crate::server::client().post(address).json(&body), &token).await;
}

#[derive(Deserialize)]
struct Taken {
    #[serde(default)]
    commands: Vec<Value>,
}

async fn take_commands(app: &AppHandle) {
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    let Ok(address) = crate::server::address(&server, "/api/devices/take") else {
        return;
    };
    let request = crate::server::client()
        .post(address)
        .json(&json!({ "id": device_id(app) }));
    let Ok(response) = crate::server::answer(request, &token).await else {
        return;
    };
    let Ok(taken) = response.json::<Taken>().await else {
        return;
    };
    for command in taken.commands {
        if let Ok(asked) = serde_json::from_value::<Asked>(command) {
            from_page(app, asked);
        }
    }
}

pub fn command_for_me(event: &str, data: &str, id: &str) -> bool {
    event == "command"
        && serde_json::from_str::<Value>(data)
            .ok()
            .and_then(|value| value.get("to").and_then(Value::as_str).map(str::to_string))
            .is_some_and(|to| to == id)
}

#[derive(Default)]
pub struct EventReader {
    event: String,
    data: String,
    buffer: String,
    pending: Vec<u8>,
}

impl EventReader {
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    text.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        if let Ok(valid) = std::str::from_utf8(&self.pending[..valid_up_to]) {
                            text.push_str(valid);
                        }
                        self.pending.drain(..valid_up_to);
                    }
                    match error.error_len() {
                        Some(bad_len) => {
                            text.push('\u{FFFD}');
                            self.pending.drain(..bad_len);
                        }
                        None => break,
                    }
                }
            }
        }
        if text.is_empty() {
            Vec::new()
        } else {
            self.feed(&text)
        }
    }

    pub fn feed(&mut self, chunk: &str) -> Vec<(String, String)> {
        self.buffer.push_str(chunk);
        let mut found = Vec::new();
        while let Some(end) = self.buffer.find('\n') {
            let line = self.buffer[..end].trim_end_matches('\r').to_string();
            self.buffer.drain(..=end);
            if line.is_empty() {
                if !self.data.is_empty() {
                    let event = if self.event.is_empty() {
                        "message".to_string()
                    } else {
                        std::mem::take(&mut self.event)
                    };
                    found.push((event, std::mem::take(&mut self.data)));
                }
                self.event.clear();
            } else if let Some(name) = line.strip_prefix("event:") {
                self.event = name.trim().to_string();
            } else if let Some(text) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(text.trim_start());
            }
        }
        found
    }
}

async fn listen_once(app: &AppHandle) {
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    let Ok(address) = crate::server::address(&server, "/api/devices/live") else {
        return;
    };
    let Ok(mut response) = crate::server::client()
        .get(address)
        .header("authorization", format!("Token {token}"))
        .header("accept", "text/event-stream")
        .send()
        .await
    else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    take_commands(app).await;
    let id = device_id(app);
    let mut reader = EventReader::default();
    while let Ok(Some(chunk)) = response.chunk().await {
        if !wanted(app) {
            return;
        }
        for (event, data) in reader.feed_bytes(&chunk) {
            if command_for_me(&event, &data, &id) {
                take_commands(app).await;
            }
        }
    }
}

pub async fn link(app: AppHandle) {
    loop {
        if wanted(&app) {
            listen_once(&app).await;
        }
        tokio::time::sleep(RETRY_AFTER).await;
    }
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
    let before = with_player(app, Player::snapshot).flatten();
    let repeat = before
        .as_ref()
        .map_or_else(|| "off".to_string(), |now| now.repeat.clone());
    let shuffle = before.is_some_and(|now| now.shuffle) || SHUFFLE.load(Ordering::Relaxed);
    let handover = Handover {
        tracks,
        position,
        time: time.max(0.0),
        playing,
        volume: settings().volume,
        muted: settings().muted,
        levelling: settings().levelling,
        repeat,
        crossfade: settings().crossfade,
        shuffle,
    };
    let state = app.state::<AppState>();
    let mut current = state.native.lock().expect("native lock");
    current.take();
    match native::start(
        server.clone(),
        token.clone(),
        handover,
        scrobbles(server, token),
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

pub fn heard(app: &AppHandle, message: &str) -> bool {
    let Some(asked) = asked(message) else {
        return false;
    };
    if let Some(page) = carried(message) {
        apply(app, page);
    }
    from_page(app, asked);
    true
}

pub fn from_page(app: &AppHandle, asked: Asked) {
    if !wanted(app) {
        return;
    }
    let send = |command: Command| {
        with_player(app, |player| player.send(command));
    };
    match asked {
        Asked::Hello => {
            reset_report();
            RUN_WAKE.notify_one();
        }
        Asked::Play => send(Command::Play),
        Asked::Pause => send(Command::Pause),
        Asked::Toggle => send(Command::Toggle),
        Asked::Next => send(Command::Next),
        Asked::Previous => send(Command::Previous),
        Asked::Seek { time } if time.is_finite() && time >= 0.0 => send(Command::SeekTo(time)),
        Asked::Volume { volume } if (0.0..=1.0).contains(&volume) => {
            {
                let mut stored = SETTINGS.lock().expect("settings lock");
                stored.volume = volume;
                stored.muted = false;
            }
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
        Asked::Shuffle { on } => {
            SHUFFLE.store(on, Ordering::Relaxed);
            send(Command::Shuffle(Some(on)));
        }
        Asked::Jump { ahead } if (1..=MOST_UP_NEXT).contains(&ahead) => send(Command::Jump(ahead)),
        Asked::Autoplay { on } => {
            let state = app.state::<AppState>();
            if state.config().autoplay != on {
                state.update(|config| config.autoplay = on);
            }
            return;
        }
        _ => {}
    }
    report_soon(app);
}

pub fn change_key(snapshot: Option<&Snapshot>) -> Option<(String, bool, String, i64, Vec<String>)> {
    snapshot.map(|now| {
        (
            now.track.track_id.clone(),
            now.playing,
            format!(
                "{}{}",
                now.repeat,
                if now.shuffle { "+shuffle" } else { "" }
            ),
            (now.volume * 100.0).round() as i64,
            now.up_next
                .iter()
                .map(|next| next.track_id.clone())
                .collect(),
        )
    })
}

#[derive(Deserialize)]
struct AlbumInfo {
    #[serde(default)]
    album: Option<String>,
}

pub fn track_path(track_id: &str) -> String {
    let mut path = String::from("/api/listen/");
    for byte in track_id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            path.push(byte as char);
        } else {
            path.push_str(&format!("%{byte:02X}"));
        }
    }
    path
}

async fn find_album(app: AppHandle, track_id: String) {
    let Some((server, token)) = crate::server::credentials(&app) else {
        return;
    };
    let Ok(info) = crate::server::get_json::<AlbumInfo>(
        &server,
        &token,
        &track_path(&track_id),
        &[("info", "")],
    )
    .await
    else {
        return;
    };
    if let Some(album) = info
        .album
        .map(|album| album.trim().to_string())
        .filter(|album| !album.is_empty())
    {
        with_player(&app, |player| player.send(Command::Album(track_id, album)));
    }
}

const RADIO_REMEMBERS: usize = 300;
const REFILL_BELOW: usize = 3;

#[derive(Deserialize)]
struct RadioSongs {
    #[serde(default)]
    songs: Vec<Track>,
}

pub fn needs_more(snapshot: &Snapshot, autoplay: bool) -> bool {
    autoplay && snapshot.repeat == "off" && snapshot.up_next.len() < REFILL_BELOW
}

pub fn radio_body(snapshot: &Snapshot, played: &VecDeque<String>) -> Value {
    let queued = || std::iter::once(&snapshot.track).chain(snapshot.up_next.iter());
    let exclude: Vec<&str> = queued()
        .filter_map(|track| track.song_id.as_deref())
        .collect();
    let skip_tracks: Vec<&str> = played
        .iter()
        .map(String::as_str)
        .chain(queued().map(|track| track.track_id.as_str()))
        .collect();
    json!({
        "trackId": snapshot.track.track_id,
        "artist": snapshot.track.artist,
        "title": snapshot.track.title,
        "continuing": true,
        "exclude": exclude,
        "skipTracks": skip_tracks,
    })
}

fn remember_played(played: &mut VecDeque<String>, track_id: &str) {
    if played.back().map(String::as_str) == Some(track_id) {
        return;
    }
    played.push_back(track_id.to_string());
    while played.len() > RADIO_REMEMBERS {
        played.pop_front();
    }
}

async fn keep_going(app: AppHandle, body: Value) {
    let Some((server, token)) = crate::server::credentials(&app) else {
        return;
    };
    let Ok(address) = crate::server::address(&server, "/api/radio") else {
        return;
    };
    let request = crate::server::client().post(address).json(&body);
    let Ok(response) = crate::server::answer(request, &token).await else {
        return;
    };
    let Ok(found) = response.json::<RadioSongs>().await else {
        return;
    };
    for track in found.songs {
        with_player(&app, |player| {
            player.send(Command::Enqueue(Box::new(track), false))
        });
    }
}

const MAX_ASKED: usize = 500;

pub async fn run(app: AppHandle) {
    let mut since_heartbeat = HEARTBEAT;
    let mut told = None;
    let mut asked: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut refilled_after: Option<String> = None;
    let mut played: VecDeque<String> = VecDeque::new();
    loop {
        let autoplay = app.state::<AppState>().config().autoplay;
        if let Some(now) = settled(&app).flatten() {
            remember_played(&mut played, &now.track.track_id);
        }
        let low = settled(&app)
            .flatten()
            .filter(|now| needs_more(now, autoplay))
            .filter(|now| refilled_after.as_deref() != Some(now.track.track_id.as_str()));
        if let Some(now) = low {
            refilled_after = Some(now.track.track_id.clone());
            async_runtime::spawn(keep_going(app.clone(), radio_body(&now, &played)));
        }
        let missing = settled(&app)
            .flatten()
            .filter(|now| now.track.album.is_none())
            .map(|now| now.track.track_id);
        if let Some(track_id) = missing {
            if !asked.contains(&track_id) {
                if asked.len() >= MAX_ASKED {
                    asked.clear();
                }
                asked.insert(track_id.clone());
                async_runtime::spawn(find_album(app.clone(), track_id));
            }
        }
        let now = settled(&app).map(|snapshot| change_key(snapshot.as_ref()));
        let due = now
            .as_ref()
            .is_some_and(|now| since_heartbeat >= HEARTBEAT || *now != told);
        if let (true, Some(now)) = (due, now) {
            since_heartbeat = Duration::ZERO;
            told = now;
            report_to_server(&app).await;
        }
        tokio::select! {
            _ = tokio::time::sleep(if active(&app) || wanted(&app) { EVERY } else { IDLE_EVERY }) => {}
            _ = RUN_WAKE.notified() => {}
        }
        since_heartbeat += EVERY;
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
    fn asks_for_more_songs_only_near_the_end_with_autoplay_on() {
        let snapshot = |left: usize, repeat: &str| Snapshot {
            track: track("now"),
            playing: true,
            time: 10.0,
            duration: Some(200.0),
            volume: 0.8,
            repeat: repeat.into(),
            shuffle: false,
            up_next: (0..left).map(|at| track(&format!("next{at}"))).collect(),
        };
        assert!(needs_more(&snapshot(0, "off"), true));
        assert!(needs_more(&snapshot(2, "off"), true));
        assert!(!needs_more(&snapshot(3, "off"), true));
        assert!(!needs_more(&snapshot(0, "off"), false));
        assert!(!needs_more(&snapshot(0, "all"), true));
        let mut played = VecDeque::new();
        remember_played(&mut played, "earlier");
        remember_played(&mut played, "now");
        remember_played(&mut played, "now");
        let body = radio_body(&snapshot(2, "off"), &played);
        assert_eq!(body["trackId"], "now");
        assert_eq!(body["continuing"], true);
        assert_eq!(body["exclude"], json!(["nd-now", "nd-next0", "nd-next1"]));
        assert_eq!(
            body["skipTracks"],
            json!(["earlier", "now", "now", "next0", "next1"])
        );
    }

    #[test]
    fn asks_the_server_about_a_song_by_its_id() {
        assert_eq!(
            track_path("massive-attack-teardrop"),
            "/api/listen/massive-attack-teardrop"
        );
        assert_eq!(track_path("a b/c?"), "/api/listen/a%20b%2Fc%3F");
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
        assert_eq!(
            asked(r#"soundscape-native {"do":"hello","settings":{"volume":0.5}}"#),
            Some(Asked::Hello)
        );
        assert_eq!(
            asked(r#"soundscape-native {"do":"seek","time":3,"settings":null}"#),
            Some(Asked::Seek { time: 3.0 })
        );
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
            shuffle: true,
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
        assert_eq!(report["state"]["shuffle"], true);
        let idle: Value =
            serde_json::from_str(&report_json("abcdefgh12", "Desktop app", None, 0.5)).unwrap();
        assert_eq!(idle["state"]["track"], Value::Null);
        assert_eq!(idle["state"]["volume"], 0.5);
        assert!(report_script("{}").contains("soundscapeNativeReport"));
    }

    #[test]
    fn follows_the_page_player_settings() {
        let page: PageSettings = serde_json::from_str(
            r#"{"volume":0.35,"muted":false,"levelling":false,"crossfade":6}"#,
        )
        .unwrap();
        let now = Settings::DEFAULT.with(page);
        assert_eq!(now.volume, 0.35);
        assert!(!now.levelling);
        assert_eq!(now.crossfade, 6.0);
        let odd: PageSettings = serde_json::from_str(r#"{"volume":null,"crossfade":5}"#).unwrap();
        let kept = now.with(odd);
        assert_eq!(kept.volume, 0.35);
        assert_eq!(kept.crossfade, 0.0);
        assert!(kept.levelling);
        assert_eq!(
            carried(r#"soundscape-native {"do":"hello","settings":{"volume":0.5,"muted":true,"levelling":true,"crossfade":0}}"#)
                .map(|page| (page.volume, page.muted)),
            Some((Some(0.5), true))
        );
        assert_eq!(carried(r#"soundscape-native {"do":"hello"}"#), None);
    }

    #[test]
    fn reads_the_server_event_stream() {
        let mut reader = EventReader::default();
        assert!(reader.feed("event: comm").is_empty());
        let found =
            reader.feed("and\ndata: {\"to\":\"abcdefgh12\"}\n\nevent: device\ndata: {}\n\n");
        assert_eq!(found.len(), 2);
        assert!(command_for_me(&found[0].0, &found[0].1, "abcdefgh12"));
        assert!(!command_for_me(&found[0].0, &found[0].1, "someone-else"));
        assert!(!command_for_me(&found[1].0, &found[1].1, "abcdefgh12"));
        let mut split = EventReader::default();
        let raw = "event: command\ndata: {\"title\":\"🎵 café\"}\n\n".as_bytes();
        let cut = raw.iter().position(|&b| b == 0xF0).unwrap() + 2;
        assert!(split.feed_bytes(&raw[..cut]).is_empty());
        let decoded = split.feed_bytes(&raw[cut..]);
        assert_eq!(
            decoded,
            vec![("command".into(), r#"{"title":"🎵 café"}"#.into())]
        );
    }

    #[test]
    fn tells_the_server_when_something_changes() {
        let now = Snapshot {
            track: track("a"),
            playing: true,
            time: 12.0,
            duration: Some(200.0),
            volume: 0.6,
            repeat: "off".into(),
            shuffle: false,
            up_next: Vec::new(),
        };
        let later = Snapshot {
            time: 15.0,
            ..now.clone()
        };
        assert_eq!(change_key(Some(&now)), change_key(Some(&later)));
        let paused = Snapshot {
            playing: false,
            ..now.clone()
        };
        assert_ne!(change_key(Some(&now)), change_key(Some(&paused)));
        let shuffled = Snapshot {
            shuffle: true,
            ..now.clone()
        };
        assert_ne!(change_key(Some(&now)), change_key(Some(&shuffled)));
        assert_ne!(change_key(Some(&now)), change_key(None));
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
        assert!(script.contains("window.soundscapeDeviceHeld = true;"));
        assert!(script.contains("soundscape:player-options"));
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
