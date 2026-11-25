use crate::host::{async_runtime, AppHandle, Manager};
use crate::media::Preview;
use crate::native::{
    self, Bounce, Command, Handover, Player, SavedQueue, SleepRequest, SleepSnapshot, Snapshot,
    Style, Track, Visualizer,
};
use crate::{AppState, MAIN};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
pub const DEFAULT_ACCENT: u32 = 0x1db954;

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
    #[serde(default = "yes")]
    pub visualizer: bool,
    #[serde(default)]
    pub visualizer_style: Style,
    #[serde(default)]
    pub visualizer_bounce: Bounce,
    #[serde(default)]
    pub visualizer_bars: usize,
    #[serde(default, deserialize_with = "hex_color")]
    pub accent: Option<u32>,
    #[serde(default)]
    pub accent_icon: bool,
}

fn hex_color<'de, D: serde::Deserializer<'de>>(found: D) -> Result<Option<u32>, D::Error> {
    Ok(Option::<String>::deserialize(found)?.and_then(|text| {
        let digits = text.strip_prefix('#')?;
        (digits.len() == 6 && digits.bytes().all(|digit| digit.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(digits, 16).ok())
            .flatten()
    }))
}

fn volume_percent(volume: f64) -> u8 {
    (volume.clamp(0.0, 1.0) * 100.0).round() as u8
}

fn volume_from_percent(percent: u8) -> f64 {
    f64::from(percent.min(100)) / 100.0
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub volume: f64,
    pub volume_known: bool,
    pub muted: bool,
    pub levelling: bool,
    pub crossfade: f64,
    pub visualizer: Visualizer,
    pub accent: u32,
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        volume: DEFAULT_VOLUME,
        volume_known: false,
        muted: false,
        levelling: true,
        crossfade: 0.0,
        visualizer: Visualizer::DEFAULT,
        accent: DEFAULT_ACCENT,
    };

    pub fn with_volume(self, volume: f64) -> Settings {
        Settings {
            volume: volume.clamp(0.0, 1.0),
            volume_known: true,
            muted: false,
            ..self
        }
    }

    pub fn with(self, page: PageSettings) -> Settings {
        let seed_volume = (!self.volume_known)
            .then_some(page.volume)
            .flatten()
            .filter(|volume| (0.0..=1.0).contains(volume));
        let (volume, muted, volume_known) = match seed_volume {
            Some(volume) => (volume, page.muted, true),
            None => (self.volume, self.muted, self.volume_known),
        };
        Settings {
            volume,
            volume_known,
            muted,
            levelling: page.levelling,
            crossfade: if CROSSFADE_CHOICES.contains(&page.crossfade) {
                page.crossfade
            } else {
                0.0
            },
            visualizer: Visualizer::new(
                page.visualizer,
                page.visualizer_style,
                page.visualizer_bounce,
                page.visualizer_bars,
            ),
            accent: page.accent.unwrap_or(DEFAULT_ACCENT),
        }
    }

    fn command(self) -> Command {
        Command::Settings {
            volume: self.volume,
            muted: self.muted,
            levelling: self.levelling,
            crossfade: self.crossfade,
            visualizer: self.visualizer,
        }
    }
}

pub fn init(saved_volume: Option<u8>) {
    if let Some(percent) = saved_volume {
        let mut stored = SETTINGS.lock().expect("settings lock");
        *stored = stored.with_volume(volume_from_percent(percent));
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
    Remove {
        ahead: usize,
    },
    Move {
        from: usize,
        to: usize,
    },
    SaveQueue {
        id: String,
        name: String,
    },
    Sleep {
        kind: String,
        #[serde(default)]
        seconds: Option<f64>,
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

pub fn volume_script(volume: f64) -> String {
    let volume = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        DEFAULT_VOLUME
    };
    let stored = json!({ "volume": volume, "muted": false }).to_string();
    format!(
        "(() => {{ try {{ localStorage.setItem(\"soundscape-volume\", {}); window.dispatchEvent(new CustomEvent(\"soundscape-set-volume\", {{ detail: {volume} }})); }} catch {{}} }})()",
        Value::from(stored),
    )
}

pub fn page_script(id: &str, name: &str) -> String {
    format!(
        r#"(() => {{
  if (window.top !== window) return;
  window.soundscapeDeviceId = {};
  window.soundscapeDeviceName = {};
  window.soundscapeDeviceHeld = true;
  window.soundscapeNativeQueueSave = true;
  const prefix = {};
  const saveVolume = (value) => {{
    if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 1) return;
    try {{
      localStorage.setItem("soundscape-volume", JSON.stringify({{ volume: value, muted: false }}));
      window.dispatchEvent(new CustomEvent("soundscape-set-volume", {{ detail: value }}));
    }} catch {{}}
  }};
  const settings = () => {{
    try {{
      const volume = JSON.parse(localStorage.getItem("soundscape-volume") ?? "null");
      const options = JSON.parse(localStorage.getItem("soundscape:player-options") ?? "null");
      return {{
        volume: typeof volume?.volume === "number" ? volume.volume : null,
        muted: volume?.muted === true,
        levelling: options?.levelling !== false,
        crossfade: typeof options?.crossfade === "number" ? options.crossfade : 0,
        visualizer: options?.visualizer !== false,
        visualizer_style: typeof options?.visualizerStyle === "string" ? options.visualizerStyle : "bars",
        visualizer_bounce: typeof options?.visualizerBounce === "string" ? options.visualizerBounce : "lively",
        visualizer_bars: [4, 6, 8, 12].includes(options?.visualizerBars) ? options.visualizerBars : 8,
        accent: typeof options?.accent === "string" ? options.accent : null,
        accent_icon: options?.accentIcon === true,
      }};
    }} catch {{
      return null;
    }}
  }};
  const post = (message) => {{
    let sent = String(message);
    try {{
      const parsed = JSON.parse(sent);
      if (parsed?.do === "volume") saveVolume(parsed.volume);
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

fn sleep_json(sleep: Option<&SleepSnapshot>) -> Value {
    match sleep {
        Some(SleepSnapshot::AfterSong) => json!({ "kind": "song" }),
        Some(SleepSnapshot::Remaining(left)) => json!({ "kind": "time", "left": left.max(0.0) }),
        None => Value::Null,
    }
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
            "sleep": sleep_json(now.sleep.as_ref()),
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
            "sleep": null,
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

pub fn queue_saved_script(detail: &Value) -> String {
    format!(
        "window.dispatchEvent(new CustomEvent(\"soundscape-queue-saved\", {{ detail: {detail} }})), undefined"
    )
}

fn settings() -> Settings {
    *SETTINGS.lock().expect("settings lock")
}

fn volume() -> f64 {
    settings().volume
}

pub fn current_volume() -> f64 {
    let stored = settings();
    if stored.muted {
        0.0
    } else {
        stored.volume.clamp(0.0, 1.0)
    }
}

pub fn set_volume(app: &AppHandle, volume: f64) -> bool {
    if !active(app) {
        return false;
    }
    let volume = if volume.is_finite() {
        volume.clamp(0.0, 1.0)
    } else {
        0.0
    };
    {
        let mut stored = SETTINGS.lock().expect("settings lock");
        *stored = stored.with_volume(volume);
    }
    with_player(app, |player| player.send(Command::Volume(volume)));
    app.host().eval(MAIN, volume_script(volume));
    save_volume_later(app);
    crate::mini::wake();
    crate::media::poke();
    true
}

pub fn set_shuffle(app: &AppHandle, on: bool) -> bool {
    if !active(app) {
        return false;
    }
    SHUFFLE.store(on, Ordering::Relaxed);
    with_player(app, |player| player.send(Command::Shuffle(Some(on))));
    report_soon(app);
    crate::mini::wake();
    crate::media::poke();
    true
}

pub fn set_repeat(app: &AppHandle, mode: &str) -> bool {
    if !active(app) || !REPEAT_MODES.contains(&mode) {
        return false;
    }
    with_player(app, |player| {
        player.send(Command::SetRepeat(mode.to_string()))
    });
    report_soon(app);
    crate::mini::wake();
    crate::media::poke();
    true
}

fn apply(app: &AppHandle, page: PageSettings) {
    let (fresh, changed) = {
        let mut stored = SETTINGS.lock().expect("settings lock");
        let fresh = stored.with(page);
        let changed = *stored != fresh;
        *stored = fresh;
        (fresh, changed)
    };
    let state = app.state::<AppState>();
    if state.config().visualizer != fresh.visualizer.on {
        state.update(|config| config.visualizer = fresh.visualizer.on);
    }
    let icon = icon_accent(page.accent_icon, fresh.accent);
    if state.config().icon_accent != icon {
        state.update(|config| config.icon_accent = icon);
        crate::tray::recolor(icon);
        app.host().icon(icon);
        crate::mini::wake();
    }
    if changed {
        with_player(app, |player| player.send(fresh.command()));
        crate::mini::wake();
    }
}

pub fn set_visualizer(app: &AppHandle, on: bool) {
    let fresh = {
        let mut stored = SETTINGS.lock().expect("settings lock");
        stored.visualizer.on = on;
        *stored
    };
    with_player(app, |player| player.send(fresh.command()));
}

pub fn icon_accent(wanted: bool, accent: u32) -> Option<u32> {
    (wanted && accent != DEFAULT_ACCENT).then_some(accent)
}

pub fn accent() -> u32 {
    settings().accent
}

pub fn visualizer(app: &AppHandle) -> Option<Visualizer> {
    let shown = settings().visualizer;
    (shown.on && active(app)).then_some(shown)
}

pub fn spectrum(app: &AppHandle) -> Option<Vec<f32>> {
    with_player(app, Player::spectrum).flatten()
}

pub fn spectrum_script(bars: Option<&[f32]>) -> String {
    let detail = match bars {
        Some(bars) => Value::from(
            bars.iter()
                .map(|v| ((*v as f64) * 1000.0).round() / 1000.0)
                .collect::<Vec<_>>(),
        )
        .to_string(),
        None => "null".into(),
    };
    format!(
        "window.dispatchEvent(new CustomEvent(\"soundscape-spectrum\", {{ detail: {detail} }})), undefined"
    )
}

static LAST_REPORT: Mutex<Option<String>> = Mutex::new(None);
static RUN_WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
static VOLUME_SAVE: AtomicU64 = AtomicU64::new(0);
const IDLE_EVERY: Duration = Duration::from_secs(5);
const VOLUME_SAVE_AFTER: Duration = Duration::from_millis(300);

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
    let json = report_json(
        &device_id(app),
        &device_name(),
        snapshot.as_ref(),
        current_volume(),
    );
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

fn save_volume_later(app: &AppHandle) {
    let ticket = VOLUME_SAVE.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    let app = app.clone();
    async_runtime::spawn(async move {
        tokio::time::sleep(VOLUME_SAVE_AFTER).await;
        if VOLUME_SAVE.load(Ordering::Relaxed) != ticket {
            return;
        }
        let percent = volume_percent(volume());
        let state = app.state::<AppState>();
        if state.config().volume != Some(percent) {
            state.update(|config| config.volume = Some(percent));
        }
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
    let json = report_json(
        &device_id(app),
        &device_name(),
        snapshot.as_ref(),
        current_volume(),
    );
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

fn start_handover(app: &AppHandle, server: Url, token: String, handover: Handover) -> bool {
    let state = app.state::<AppState>();
    let mut current = state.native.lock().expect("native lock");
    current.take();
    match native::start(
        server.clone(),
        token.clone(),
        handover,
        scrobbles(server, token),
    ) {
        Ok(player) => {
            *current = Some(player);
            true
        }
        Err(error) => {
            eprintln!("The built-in player couldn't start: {error}");
            false
        }
    }
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
        restored: false,
        unshuffled: None,
        visualizer: settings().visualizer,
    };
    start_handover(app, server, token, handover);
}

const QUEUE_FILE: &str = "player-queue.json";
const SAVE_QUEUE_EVERY: Duration = Duration::from_secs(15);
static QUEUE_IO: Mutex<()> = Mutex::new(());

pub fn queue_path(dir: &Path) -> PathBuf {
    dir.join(QUEUE_FILE)
}

pub fn save_saved_queue(dir: &Path, saved: &SavedQueue) -> std::io::Result<()> {
    let _guard = QUEUE_IO.lock().expect("queue io lock");
    fs::create_dir_all(dir)?;
    let path = queue_path(dir);
    let temporary = dir.join(format!("player-queue.json.new.{}", std::process::id()));
    let bytes = serde_json::to_vec(saved)?;
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

pub fn clear_saved_queue(dir: &Path) {
    let _guard = QUEUE_IO.lock().expect("queue io lock");
    let _ = fs::remove_file(queue_path(dir));
}

pub fn load_saved_queue(dir: &Path, server: &Url) -> Option<SavedQueue> {
    let _guard = QUEUE_IO.lock().expect("queue io lock");
    let text = fs::read_to_string(queue_path(dir)).ok()?;
    let saved: SavedQueue = serde_json::from_str(&text).ok()?;
    if saved.server != server.as_str() {
        return None;
    }
    let (tracks, position) = queue_window(saved.tracks, saved.position)?;
    let repeat = if REPEAT_MODES.contains(&saved.repeat.as_str()) {
        saved.repeat
    } else {
        "off".into()
    };
    let time = if saved.time.is_finite() && saved.time > 0.0 {
        saved.time
    } else {
        0.0
    };
    let unshuffled = if saved.shuffle {
        saved.unshuffled
    } else {
        None
    };
    Some(SavedQueue {
        server: saved.server,
        tracks,
        position,
        time,
        repeat,
        shuffle: saved.shuffle,
        unshuffled,
    })
}

pub fn save_now(app: &AppHandle) {
    let Some((server, _)) = crate::server::credentials(app) else {
        return;
    };
    let Some(Some(saved)) = with_player(app, |player| player.saved_state(server.as_str())) else {
        return;
    };
    let dir = app.state::<AppState>().dir.clone();
    let _ = save_saved_queue(&dir, &saved);
}

pub fn clear_saved(app: &AppHandle) {
    let dir = app.state::<AppState>().dir.clone();
    clear_saved_queue(&dir);
}

pub fn restore(app: &AppHandle) {
    if !wanted(app) || active(app) {
        return;
    }
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    let dir = app.state::<AppState>().dir.clone();
    let Some(saved) = load_saved_queue(&dir, &server) else {
        return;
    };
    SHUFFLE.store(saved.shuffle, Ordering::Relaxed);
    let handover = Handover {
        tracks: saved.tracks,
        position: saved.position,
        time: saved.time,
        playing: false,
        volume: settings().volume,
        muted: settings().muted,
        levelling: settings().levelling,
        repeat: saved.repeat,
        crossfade: settings().crossfade,
        shuffle: saved.shuffle,
        restored: true,
        unshuffled: saved.unshuffled,
        visualizer: settings().visualizer,
    };
    if start_handover(app, server, token, handover) {
        crate::media::poke();
        crate::mini::wake();
        report_soon(app);
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

fn save_queue_as_playlist(app: &AppHandle, id: String, name: String) {
    let fail = |message: &str| {
        let detail = json!({ "id": id, "ok": false, "error": message });
        app.host().eval(MAIN, queue_saved_script(&detail));
    };
    let name = name.trim().to_string();
    if name.is_empty() {
        fail("Give the playlist a name.");
        return;
    }
    let tracks = with_player(app, Player::queue_tracks).unwrap_or_default();
    if tracks.is_empty() {
        fail("The queue is empty.");
        return;
    }
    let Some((server, token)) = crate::server::credentials(app) else {
        fail("Log in to save playlists.");
        return;
    };
    let payload_tracks: Vec<Value> = tracks
        .into_iter()
        .take(MOST_TRACKS)
        .map(|track| {
            let mut item = json!({
                "trackId": track.track_id,
                "title": track.title,
                "artist": track.artist,
            });
            if let Some(song_id) = track.song_id {
                item["songId"] = Value::from(song_id);
            }
            item
        })
        .collect();
    let body = json!({ "name": name, "tracks": payload_tracks });
    let app = app.clone();
    async_runtime::spawn(async move {
        let detail = match crate::server::address(&server, "/api/music/queue-playlist") {
            Err(_) => json!({ "id": id, "ok": false, "error": "Could not save playlist." }),
            Ok(address) => {
                let sent = crate::server::client()
                    .post(address)
                    .timeout(Duration::from_secs(30))
                    .header("authorization", format!("Token {token}"))
                    .json(&body)
                    .send()
                    .await;
                match sent {
                    Err(_) => {
                        json!({ "id": id, "ok": false, "error": "Could not reach Soundscape." })
                    }
                    Ok(response) => {
                        let ok = response.status().is_success();
                        let answered = response.json::<Value>().await.ok();
                        if ok {
                            let saved = answered
                                .as_ref()
                                .and_then(|v| v.get("saved"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            let total = answered
                                .as_ref()
                                .and_then(|v| v.get("total"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            let missing = answered
                                .as_ref()
                                .and_then(|v| v.get("missing"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            let saved_name = answered
                                .as_ref()
                                .and_then(|v| v.get("name"))
                                .and_then(Value::as_str)
                                .unwrap_or(&name);
                            json!({
                                "id": id,
                                "ok": true,
                                "name": saved_name,
                                "saved": saved,
                                "total": total,
                                "missing": missing,
                            })
                        } else {
                            let message = answered
                                .as_ref()
                                .and_then(|v| v.get("error"))
                                .and_then(Value::as_str)
                                .unwrap_or("Could not save playlist.");
                            json!({ "id": id, "ok": false, "error": message })
                        }
                    }
                }
            }
        };
        app.host().eval(MAIN, queue_saved_script(&detail));
    });
}

pub fn heard(app: &AppHandle, message: &str) -> bool {
    let Some(asked) = asked(message) else {
        return false;
    };
    let setting_volume = matches!(asked, Asked::Volume { volume } if (0.0..=1.0).contains(&volume));
    if let Some(mut page) = carried(message) {
        if setting_volume {
            page.volume = None;
        }
        apply(app, page);
    }
    handle_asked(app, asked, false);
    true
}

pub fn from_page(app: &AppHandle, asked: Asked) {
    handle_asked(app, asked, true);
}

fn handle_asked(app: &AppHandle, asked: Asked, sync_page: bool) {
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
                *stored = stored.with_volume(volume);
            }
            send(Command::Volume(volume));
            if sync_page {
                app.host().eval(MAIN, volume_script(volume));
            }
            save_volume_later(app);
            crate::mini::wake();
            crate::media::poke();
            return;
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
        Asked::Remove { ahead } if (1..=MOST_UP_NEXT).contains(&ahead) => {
            send(Command::Remove(ahead))
        }
        Asked::Move { from, to }
            if (1..=MOST_UP_NEXT).contains(&from)
                && (1..=MOST_UP_NEXT).contains(&to)
                && from != to =>
        {
            send(Command::Move(from, to))
        }
        Asked::SaveQueue { id, name } => {
            save_queue_as_playlist(app, id, name);
            return;
        }
        Asked::Sleep { kind, seconds } => {
            let request = match kind.as_str() {
                "song" => Some(SleepRequest::AfterSong),
                "time" => seconds
                    .filter(|s| s.is_finite() && *s >= 1.0)
                    .map(SleepRequest::Seconds),
                _ => None,
            };
            send(Command::Sleep(request));
        }
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
    crate::mini::wake();
    crate::media::poke();
}

fn sleep_key(sleep: Option<&SleepSnapshot>) -> Option<String> {
    match sleep {
        Some(SleepSnapshot::AfterSong) => Some("song".into()),
        Some(SleepSnapshot::Remaining(left)) => {
            Some(format!("time:{}", (left / 60.0).ceil() as i64))
        }
        None => None,
    }
}

pub type ChangeKey = (String, bool, String, i64, Option<String>, Vec<String>);

pub fn change_key(snapshot: Option<&Snapshot>) -> Option<ChangeKey> {
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
            sleep_key(now.sleep.as_ref()),
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
    restore(&app);
    async_runtime::spawn(stream_spectrum(app.clone()));
    let mut since_heartbeat = HEARTBEAT;
    let mut since_queue_save = Duration::ZERO;
    let mut told = None;
    let mut asked: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut refilled_after: Option<String> = None;
    let mut played: VecDeque<String> = VecDeque::new();
    loop {
        let autoplay = app.state::<AppState>().config().autoplay;
        let current_snapshot = settled(&app).flatten();
        if let Some(ref now) = current_snapshot {
            remember_played(&mut played, &now.track.track_id);
        }
        let low = current_snapshot
            .as_ref()
            .filter(|now| needs_more(now, autoplay))
            .filter(|now| refilled_after.as_deref() != Some(now.track.track_id.as_str()));
        if let Some(now) = low {
            refilled_after = Some(now.track.track_id.clone());
            async_runtime::spawn(keep_going(app.clone(), radio_body(now, &played)));
        }
        let missing = current_snapshot
            .as_ref()
            .filter(|now| now.track.album.is_none())
            .map(|now| now.track.track_id.clone());
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
        let changed = now.as_ref().is_some_and(|key| *key != told);
        let save_due = changed
            || (since_queue_save >= SAVE_QUEUE_EVERY
                && current_snapshot.as_ref().is_some_and(|s| s.playing));
        if save_due && active(&app) {
            since_queue_save = Duration::ZERO;
            save_now(&app);
        }
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
        since_queue_save += EVERY;
        if active(&app) {
            report(&app);
        }
    }
}

async fn stream_spectrum(app: AppHandle) {
    const WAITING: Duration = Duration::from_millis(250);
    let mut last: Option<Vec<f32>> = None;
    loop {
        let now = spectrum(&app);
        if now != last {
            if now.is_some() != last.is_some() {
                crate::mini::wake();
            }
            if app.host().is_visible(MAIN) {
                app.host().eval(MAIN, spectrum_script(now.as_deref()));
            }
            last = now;
        }
        let wait = if last.is_some() {
            native::SPECTRUM_TICK
        } else if active(&app) {
            WAITING
        } else {
            EVERY
        };
        tokio::time::sleep(wait).await;
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
            sleep: None,
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
            sleep: Some(SleepSnapshot::Remaining(120.0)),
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
        assert_eq!(report["state"]["sleep"]["kind"], "time");
        assert_eq!(report["state"]["sleep"]["left"], 120.0);
        let idle: Value =
            serde_json::from_str(&report_json("abcdefgh12", "Desktop app", None, 0.5)).unwrap();
        assert_eq!(idle["state"]["track"], Value::Null);
        assert_eq!(idle["state"]["volume"], 0.5);
        assert_eq!(idle["state"]["sleep"], Value::Null);
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
        assert!(now.volume_known);
        assert!(!now.levelling);
        assert_eq!(now.crossfade, 6.0);
        let odd: PageSettings = serde_json::from_str(r#"{"volume":null,"crossfade":5}"#).unwrap();
        let kept = now.with(odd);
        assert_eq!(kept.volume, 0.35);
        assert_eq!(kept.crossfade, 0.0);
        assert!(kept.levelling);
        let lowered = kept.with_volume(0.1);
        let stale: PageSettings = serde_json::from_str(
            r##"{"volume":1.0,"muted":true,"levelling":false,"crossfade":4,"visualizer":false,"accent":"#8B5CF6"}"##,
        )
        .unwrap();
        let reopened = lowered.with(stale);
        assert_eq!(reopened.volume, 0.1);
        assert!(!reopened.muted);
        assert!(reopened.volume_known);
        assert!(!reopened.levelling);
        assert_eq!(reopened.crossfade, 4.0);
        assert!(!reopened.visualizer.on);
        assert_eq!(reopened.accent, 0x8b5cf6);
        assert_eq!(volume_from_percent(volume_percent(0.1)), 0.1);
        assert_eq!(volume_percent(1.5), 100);
        assert_eq!(volume_from_percent(150), 1.0);
        assert_eq!(
            carried(r#"soundscape-native {"do":"hello","settings":{"volume":0.5,"muted":true,"levelling":true,"crossfade":0}}"#)
                .map(|page| (page.volume, page.muted)),
            Some((Some(0.5), true))
        );
        assert_eq!(carried(r#"soundscape-native {"do":"hello"}"#), None);
        assert_eq!(now.accent, DEFAULT_ACCENT);
        let accent = |json: &str| {
            Settings::DEFAULT
                .with(serde_json::from_str(json).unwrap())
                .accent
        };
        assert_eq!(accent(r##"{"accent":"#8B5CF6"}"##), 0x8b5cf6);
        assert_eq!(accent(r#"{"accent":"purple"}"#), DEFAULT_ACCENT);
        assert_eq!(accent(r##"{"accent":"#12345"}"##), DEFAULT_ACCENT);
        assert_eq!(accent(r#"{"accent":null}"#), DEFAULT_ACCENT);
        assert_eq!(icon_accent(true, 0x8b5cf6), Some(0x8b5cf6));
        assert_eq!(icon_accent(false, 0x8b5cf6), None);
        assert_eq!(icon_accent(true, DEFAULT_ACCENT), None);
        let page: PageSettings = serde_json::from_str(r#"{"accent_icon":true}"#).unwrap();
        assert!(page.accent_icon);
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
            sleep: None,
            up_next: Vec::new(),
        };
        let later = Snapshot {
            time: 15.0,
            ..now.clone()
        };
        assert_eq!(change_key(Some(&now)), change_key(Some(&later)));
        let sleeping = Snapshot {
            sleep: Some(SleepSnapshot::AfterSong),
            ..now.clone()
        };
        assert_ne!(change_key(Some(&now)), change_key(Some(&sleeping)));
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
    fn reads_sleep_commands_from_the_page() {
        assert_eq!(
            asked(r#"soundscape-native {"do":"sleep","kind":"off"}"#),
            Some(Asked::Sleep {
                kind: "off".into(),
                seconds: None
            })
        );
        assert_eq!(
            asked(r#"soundscape-native {"do":"sleep","kind":"song"}"#),
            Some(Asked::Sleep {
                kind: "song".into(),
                seconds: None
            })
        );
        assert_eq!(
            asked(r#"soundscape-native {"do":"sleep","kind":"time","seconds":900}"#),
            Some(Asked::Sleep {
                kind: "time".into(),
                seconds: Some(900.0)
            })
        );
    }

    #[test]
    fn saves_and_loads_the_last_queue_privately_per_server() {
        let dir = std::env::temp_dir().join(format!(
            "soundscape-desktop-queue-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let server = Url::parse("https://music.example.com/").unwrap();
        let other = Url::parse("https://other.example.com/").unwrap();
        let saved = SavedQueue {
            server: server.as_str().into(),
            tracks: vec![track("a"), track("b")],
            position: 1,
            time: 37.5,
            repeat: "all".into(),
            shuffle: true,
            unshuffled: Some(vec![("a".into(), Some("nd-a".into()))]),
        };
        save_saved_queue(&dir, &saved).unwrap();
        let mode = fs::metadata(queue_path(&dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(load_saved_queue(&dir, &server), Some(saved));
        assert_eq!(load_saved_queue(&dir, &other), None);
        clear_saved_queue(&dir);
        assert_eq!(load_saved_queue(&dir, &server), None);
        fs::write(queue_path(&dir), "not json").unwrap();
        assert_eq!(load_saved_queue(&dir, &server), None);
        let _ = fs::remove_dir_all(&dir);
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
        assert!(script.contains("window.soundscapeNativeQueueSave = true;"));
        assert!(script.contains("soundscape:player-options"));
        assert!(script.contains("soundscape-volume"));
        assert!(script.contains("soundscape-set-volume"));
        let sync = volume_script(0.1);
        assert!(sync.contains("soundscape-volume"));
        assert!(sync.contains("soundscape-set-volume"));
        assert!(sync.contains("0.1"));
        let saved = queue_saved_script(&json!({ "id": "1", "ok": true, "saved": 4 }));
        assert!(saved.contains("soundscape-queue-saved"));
        assert!(saved.contains("\"saved\":4"));
    }

    #[test]
    fn reads_queue_management_and_save_commands_from_the_page() {
        assert_eq!(
            asked(r#"soundscape-native {"do":"remove","ahead":2}"#),
            Some(Asked::Remove { ahead: 2 })
        );
        assert_eq!(
            asked(r#"soundscape-native {"do":"move","from":3,"to":1}"#),
            Some(Asked::Move { from: 3, to: 1 })
        );
        assert_eq!(
            asked(r#"soundscape-native {"do":"savequeue","id":"req-1","name":"Late Night"}"#),
            Some(Asked::SaveQueue {
                id: "req-1".into(),
                name: "Late Night".into(),
            })
        );
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
