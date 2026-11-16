use crate::media::{Preview, UpNext};
use crate::relay::Relay;
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::Deserialize;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;

const TICK: Duration = Duration::from_millis(250);
const RESTART_AFTER_SECONDS: f64 = 3.0;
const SHORTEST_SONG_SECONDS: f64 = 30.0;
const LONGEST_WAIT_SECONDS: f64 = 240.0;
const LONGEST_STEP_SECONDS: f64 = 2.0;
const RELEASE_AFTER: Duration = Duration::from_secs(5 * 60);
const MOST_FAILURES_IN_A_ROW: u32 = 5;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Gain {
    pub track: Option<f64>,
    pub album: Option<f64>,
    #[serde(rename = "trackPeak")]
    pub track_peak: Option<f64>,
    #[serde(rename = "albumPeak")]
    pub album_peak: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Track {
    #[serde(rename = "trackId")]
    pub track_id: String,
    #[serde(default, rename = "songId")]
    pub song_id: Option<String>,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub gain: Option<Gain>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Handover {
    pub tracks: Vec<Track>,
    pub position: usize,
    pub time: f64,
    pub playing: bool,
    pub volume: f64,
    pub muted: bool,
    pub levelling: bool,
    pub repeat: String,
}

pub const READ_HANDOVER: &str = r#"(() => {
  const audio = document.querySelector(".player audio");
  let queue = null;
  let volume = null;
  let options = null;
  try {
    queue = JSON.parse(localStorage.getItem("soundscape:queue") ?? "null");
    volume = JSON.parse(localStorage.getItem("soundscape-volume") ?? "null");
    options = JSON.parse(localStorage.getItem("soundscape:player-options") ?? "null");
  } catch {}
  if (!audio || !audio.currentSrc || !queue || !Array.isArray(queue.order) || !Array.isArray(queue.tracks)) return "null";
  return JSON.stringify({
    tracks: queue.order.map((index) => queue.tracks[index]).filter(Boolean),
    position: queue.position,
    time: Number.isFinite(audio.currentTime) ? audio.currentTime : 0,
    playing: !audio.paused && !audio.ended,
    volume: typeof volume?.volume === "number" ? volume.volume : 0.8,
    muted: Boolean(volume?.muted),
    levelling: options?.levelling !== false,
    repeat: document.querySelector('[data-player="repeat"]')?.dataset.repeat ?? "off",
  });
})()"#;

pub const PAUSE_PAGE: &str =
    r#"(() => { for (const audio of document.querySelectorAll("audio")) audio.pause(); })()"#;

pub const RESUME_PAGE: &str =
    r#"(() => { document.querySelector(".player audio")?.play().catch(() => {}); })()"#;

pub fn parse_handover(json: &str) -> Option<Handover> {
    let handover = serde_json::from_str::<Option<Handover>>(json).ok()??;
    (handover.position < handover.tracks.len()).then_some(handover)
}

pub fn gain_level(gain: Option<&Gain>, by_album: bool) -> f64 {
    let Some(gain) = gain else { return 1.0 };
    let use_album = by_album && gain.album.is_some();
    let db = if use_album {
        gain.album
    } else {
        gain.track.or(gain.album)
    };
    let Some(db) = db else { return 1.0 };
    let peak = if use_album {
        gain.album_peak.or(gain.track_peak)
    } else {
        gain.track_peak.or(gain.album_peak)
    };
    let level = 10f64
        .powf(db / 20.0)
        .min(
            peak.filter(|peak| *peak > 0.0)
                .map_or(1.0, |peak| 1.0 / peak),
        )
        .min(1.0);
    (level * 1000.0).round() / 1000.0
}

fn same_album(a: Option<&Track>, b: Option<&Track>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.album.is_some() && a.album == b.album && a.artist == b.artist,
        _ => false,
    }
}

pub fn playing_an_album(tracks: &[Track], at: usize) -> bool {
    let here = tracks.get(at);
    same_album(here, at.checked_sub(1).and_then(|i| tracks.get(i)))
        || same_album(here, tracks.get(at + 1))
}

pub fn next_index(at: usize, length: usize, repeat: &str) -> Option<usize> {
    if repeat == "one" {
        return Some(at);
    }
    if at + 1 < length {
        Some(at + 1)
    } else if repeat == "all" && length > 1 {
        Some(0)
    } else {
        None
    }
}

pub fn next_repeat(repeat: &str) -> &'static str {
    match repeat {
        "off" => "all",
        "all" => "one",
        _ => "off",
    }
}

pub fn counts_as_play(heard: f64, length: f64) -> bool {
    length.is_finite()
        && length >= SHORTEST_SONG_SECONDS
        && heard >= (length / 2.0).min(LONGEST_WAIT_SECONDS)
}

pub fn heard_step(before: Option<f64>, now: f64) -> f64 {
    match before {
        Some(before) if now > before && now - before <= LONGEST_STEP_SECONDS => now - before,
        _ => 0.0,
    }
}

pub fn source_for(relay: &Relay, track: &Track) -> Option<Url> {
    match &track.song_id {
        Some(song) => relay.address("full-song", song),
        None => relay.address("listen", &track.track_id),
    }
}

pub fn cover_for(server: &Url, track: &Track) -> Option<String> {
    let cover = track.cover.as_deref()?;
    if cover.starts_with("https://") || cover.starts_with("http://") {
        return Some(cover.to_string());
    }
    cover
        .starts_with('/')
        .then(|| server.join(cover).ok().map(String::from))
        .flatten()
}

pub fn handback_script(state: &Resume, nonce: u64) -> String {
    format!(
        r#"(() => {{
  try {{
    if (sessionStorage.getItem("soundscape:handback") === "{nonce}") return;
    sessionStorage.setItem("soundscape:handback", "{nonce}");
    const queue = JSON.parse(localStorage.getItem("soundscape:queue") ?? "null");
    if (!queue) return;
    queue.position = {position};
    queue.time = {time};
    localStorage.setItem("soundscape:queue", JSON.stringify(queue));
    if ({playing}) sessionStorage.setItem("soundscape:resume", "1");
    else sessionStorage.removeItem("soundscape:resume");
  }} catch {{}}
}})()"#,
        position = state.position,
        time = (state.time * 10.0).round() / 10.0,
        playing = state.playing,
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resume {
    pub position: usize,
    pub time: f64,
    pub playing: bool,
}

pub enum Command {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    SeekBy(f64),
    SeekTo(f64),
    Repeat,
    Stop,
}

pub fn command_for(action: &str, seconds: f64) -> Option<Command> {
    Some(match action {
        "play" => Command::Play,
        "pause" => Command::Pause,
        "toggle" => Command::Toggle,
        "stop" => Command::Pause,
        "next" => Command::Next,
        "previous" => Command::Previous,
        "seek" => Command::SeekBy(seconds),
        "position" => Command::SeekTo(seconds),
        "repeat" => Command::Repeat,
        _ => return None,
    })
}

struct Queue {
    tracks: Vec<Track>,
    at: usize,
    repeat: String,
    coming: Option<usize>,
}

struct Shown {
    preview: Option<Preview>,
    resume: Resume,
}

pub struct Player {
    commands: Sender<Command>,
    shown: Arc<Mutex<Shown>>,
}

impl Player {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn preview(&self) -> Option<Preview> {
        self.shown.lock().expect("shown lock").preview.clone()
    }

    pub fn resume(&self) -> Resume {
        self.shown.lock().expect("shown lock").resume.clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

pub type Report = Box<dyn Fn(&'static str, Track) + Send>;

struct Engine {
    playbin: gst::Element,
    queue: Arc<Mutex<Queue>>,
    shown: Arc<Mutex<Shown>>,
    server: Url,
    relay: Relay,
    volume: f64,
    muted: bool,
    levelling: bool,
    want_playing: bool,
    seek_when_ready: Option<f64>,
    released: bool,
    resume_time: f64,
    paused_since: Option<Instant>,
    heard: f64,
    last: Option<f64>,
    counted: bool,
    failures: u32,
    report: Report,
}

impl Engine {
    fn track(&self) -> Option<Track> {
        let queue = self.queue.lock().expect("queue lock");
        queue.tracks.get(queue.at).cloned()
    }

    fn apply_volume(&self) {
        let queue = self.queue.lock().expect("queue lock");
        let level = if self.levelling {
            gain_level(
                queue
                    .tracks
                    .get(queue.at)
                    .and_then(|track| track.gain.as_ref()),
                playing_an_album(&queue.tracks, queue.at),
            )
        } else {
            1.0
        };
        self.playbin
            .set_property("volume", (self.volume * level).clamp(0.0, 1.0));
        self.playbin.set_property("mute", self.muted);
    }

    fn started(&mut self) {
        self.heard = 0.0;
        self.last = None;
        self.counted = false;
        self.apply_volume();
        if let Some(track) = self.track() {
            if track.song_id.is_some() {
                (self.report)("now-playing", track);
            }
        }
    }

    fn load(&mut self, at: usize, time: f64) {
        let Some(uri) = ({
            let mut queue = self.queue.lock().expect("queue lock");
            queue.at = at;
            queue.coming = None;
            queue
                .tracks
                .get(at)
                .and_then(|track| source_for(&self.relay, track))
        }) else {
            return;
        };
        let _ = self.playbin.set_state(gst::State::Null);
        self.playbin.set_property("uri", uri.as_str());
        self.seek_when_ready = (time > 0.5).then_some(time);
        self.released = false;
        let state = if self.want_playing {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        let _ = self.playbin.set_state(state);
        self.started();
    }

    fn position(&self) -> f64 {
        if self.released {
            return self.resume_time;
        }
        self.playbin
            .query_position::<gst::ClockTime>()
            .map_or(self.seek_when_ready.unwrap_or(0.0), |time| {
                time.nseconds() as f64 / 1e9
            })
    }

    fn length(&self) -> Option<f64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|time| time.nseconds() as f64 / 1e9)
            .filter(|seconds| *seconds > 0.0)
    }

    fn seek(&mut self, seconds: f64) {
        let seconds = match self.length() {
            Some(length) => seconds.clamp(0.0, length),
            None => seconds.max(0.0),
        };
        if self.released {
            self.resume_time = seconds;
            return;
        }
        let target = gst::ClockTime::from_nseconds((seconds * 1e9) as u64);
        if self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target)
            .is_err()
        {
            self.seek_when_ready = Some(seconds);
        }
        self.last = None;
    }

    fn set_playing(&mut self, playing: bool) {
        self.want_playing = playing;
        if playing && self.released {
            let at = self.queue.lock().expect("queue lock").at;
            let time = self.resume_time;
            self.load(at, time);
            return;
        }
        let _ = self.playbin.set_state(if playing {
            gst::State::Playing
        } else {
            gst::State::Paused
        });
        self.paused_since = (!playing).then(Instant::now);
    }

    fn move_by(&mut self, step: i64) {
        let (at, length, repeat) = {
            let queue = self.queue.lock().expect("queue lock");
            (queue.at, queue.tracks.len(), queue.repeat.clone())
        };
        let target = if step > 0 {
            let repeat = if repeat == "one" {
                "all".to_string()
            } else {
                repeat
            };
            next_index(at, length, &repeat)
        } else if self.position() > RESTART_AFTER_SECONDS || at == 0 {
            self.seek(0.0);
            return;
        } else {
            Some(at - 1)
        };
        if let Some(target) = target {
            self.load(target, 0.0);
        }
    }

    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Play => self.set_playing(true),
            Command::Pause => self.set_playing(false),
            Command::Toggle => self.set_playing(!self.want_playing),
            Command::Next => self.move_by(1),
            Command::Previous => self.move_by(-1),
            Command::SeekBy(step) => {
                let now = self.position();
                self.seek(now + step);
            }
            Command::SeekTo(seconds) => self.seek(seconds),
            Command::Repeat => {
                let mut queue = self.queue.lock().expect("queue lock");
                queue.repeat = next_repeat(&queue.repeat).to_string();
            }
            Command::Stop => {
                let _ = self.playbin.set_state(gst::State::Null);
                return false;
            }
        }
        true
    }

    fn messages(&mut self) {
        let Some(bus) = self.playbin.bus() else {
            return;
        };
        while let Some(message) = bus.timed_pop(gst::ClockTime::ZERO) {
            match message.view() {
                gst::MessageView::StreamStart(_) => {
                    let moved = {
                        let mut queue = self.queue.lock().expect("queue lock");
                        match queue.coming.take() {
                            Some(next) => {
                                queue.at = next;
                                true
                            }
                            None => false,
                        }
                    };
                    if moved {
                        self.started();
                    }
                    self.failures = 0;
                }
                gst::MessageView::Eos(_) => {
                    self.want_playing = false;
                    let _ = self.playbin.set_state(gst::State::Paused);
                    self.paused_since = Some(Instant::now());
                }
                gst::MessageView::Error(_) => {
                    self.failures += 1;
                    let (at, length, repeat) = {
                        let queue = self.queue.lock().expect("queue lock");
                        (queue.at, queue.tracks.len(), queue.repeat.clone())
                    };
                    let repeat = if repeat == "one" {
                        "off".to_string()
                    } else {
                        repeat
                    };
                    match next_index(at, length, &repeat) {
                        Some(next) if self.failures < MOST_FAILURES_IN_A_ROW => {
                            self.load(next, 0.0)
                        }
                        _ => {
                            self.want_playing = false;
                            let _ = self.playbin.set_state(gst::State::Null);
                            self.released = true;
                            self.resume_time = 0.0;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn seek_when_ready(&mut self) {
        let Some(seconds) = self.seek_when_ready else {
            return;
        };
        if self.released || self.length().is_none() {
            return;
        }
        let target = gst::ClockTime::from_nseconds((seconds * 1e9) as u64);
        if self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target)
            .is_ok()
        {
            self.seek_when_ready = None;
            self.last = None;
        }
    }

    fn listen(&mut self) {
        if !self.want_playing || self.released {
            self.last = None;
            return;
        }
        let now = self.position();
        self.heard += heard_step(self.last, now);
        self.last = Some(now);
        if self.counted {
            return;
        }
        let Some(track) = self.track() else { return };
        if track.song_id.is_some() && counts_as_play(self.heard, self.length().unwrap_or(0.0)) {
            self.counted = true;
            (self.report)("play", track);
        }
    }

    fn release_when_quiet(&mut self) {
        if self.released || self.want_playing {
            return;
        }
        if self
            .paused_since
            .is_some_and(|since| since.elapsed() >= RELEASE_AFTER)
        {
            self.resume_time = self.position();
            let _ = self.playbin.set_state(gst::State::Null);
            self.released = true;
        }
    }

    fn publish(&self) {
        let queue = self.queue.lock().expect("queue lock");
        let Some(track) = queue.tracks.get(queue.at) else {
            return;
        };
        let next = next_index(queue.at, queue.tracks.len(), &queue.repeat)
            .filter(|_| queue.repeat != "one")
            .and_then(|at| queue.tracks.get(at));
        let position = self.position();
        let preview = Preview {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            cover: cover_for(&self.server, track),
            playing: self.want_playing && !self.released,
            position,
            length: self.length(),
            has_next: next_index(
                queue.at,
                queue.tracks.len(),
                if queue.repeat == "one" {
                    "all"
                } else {
                    &queue.repeat
                },
            )
            .is_some(),
            has_previous: true,
            shuffle: None,
            repeat: Some(queue.repeat.clone()),
            up_next: next.map(|track| UpNext {
                title: track.title.clone(),
                artist: track.artist.clone(),
            }),
        };
        let resume = Resume {
            position: queue.at,
            time: position,
            playing: self.want_playing && !self.released,
        };
        drop(queue);
        let mut shown = self.shown.lock().expect("shown lock");
        shown.preview = Some(preview);
        shown.resume = resume;
    }
}

fn audio_sink() -> Option<gst::Element> {
    let description = std::env::var("SOUNDSCAPE_AUDIO_SINK").ok()?;
    gst::parse_launch(&description).ok()
}

pub fn start(
    server: Url,
    token: String,
    handover: Handover,
    report: Report,
) -> Result<Player, String> {
    gst::init().map_err(|error| error.to_string())?;
    let playbin = gst::ElementFactory::make("playbin")
        .build()
        .map_err(|_| "GStreamer's playbin is missing".to_string())?;
    if let Some(sink) = audio_sink() {
        playbin.set_property("audio-sink", &sink);
    }
    if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
        playbin.set_property("video-sink", &fake);
    }
    let relay = Relay::start(server.clone(), token)?;

    let queue = Arc::new(Mutex::new(Queue {
        tracks: handover.tracks.clone(),
        at: handover.position,
        repeat: handover.repeat.clone(),
        coming: None,
    }));
    let next_queue = queue.clone();
    let next_relay = relay.clone();
    playbin.connect("about-to-finish", false, move |values| {
        let Ok(playbin) = values[0].get::<gst::Element>() else {
            return None;
        };
        let mut queue = next_queue.lock().expect("queue lock");
        if let Some(next) = next_index(queue.at, queue.tracks.len(), &queue.repeat) {
            if let Some(uri) = queue
                .tracks
                .get(next)
                .and_then(|track| source_for(&next_relay, track))
            {
                playbin.set_property("uri", uri.as_str());
                queue.coming = Some(next);
            }
        }
        None
    });

    let shown = Arc::new(Mutex::new(Shown {
        preview: None,
        resume: Resume {
            position: handover.position,
            time: handover.time,
            playing: handover.playing,
        },
    }));
    let (commands, receive) = channel::<Command>();
    let mut engine = Engine {
        playbin,
        queue,
        shown: shown.clone(),
        server,
        relay,
        volume: handover.volume.clamp(0.0, 1.0),
        muted: handover.muted,
        levelling: handover.levelling,
        want_playing: handover.playing,
        seek_when_ready: None,
        released: false,
        resume_time: 0.0,
        paused_since: (!handover.playing).then(Instant::now),
        heard: 0.0,
        last: None,
        counted: false,
        failures: 0,
        report,
    };
    std::thread::Builder::new()
        .name("soundscape-player".into())
        .spawn(move || {
            engine.load(handover.position, handover.time);
            loop {
                match receive.recv_timeout(TICK) {
                    Ok(command) => {
                        if !engine.handle(command) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = engine.playbin.set_state(gst::State::Null);
                        break;
                    }
                }
                engine.messages();
                engine.seek_when_ready();
                engine.listen();
                engine.release_when_quiet();
                engine.publish();
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Player { commands, shown })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, album: Option<&str>) -> Track {
        Track {
            track_id: format!("band-{}", title.to_lowercase()),
            song_id: Some(format!("nd-{title}")),
            title: title.into(),
            artist: "Band".into(),
            album: album.map(str::to_string),
            cover: Some("/api/navidrome/cover/al-1".into()),
            gain: None,
        }
    }

    #[test]
    fn reads_the_queue_the_page_hands_over() {
        let json = r#"{"tracks":[{"trackId":"a-1","songId":"nd-1","title":"One","artist":"A","album":"X","gain":{"track":-6,"album":-4,"trackPeak":0.9}},{"trackId":"a-2","title":"Two","artist":"A"}],"position":1,"time":12.5,"playing":true,"volume":0.7,"muted":false,"levelling":true,"repeat":"all"}"#;
        let handover = parse_handover(json).unwrap();
        assert_eq!(handover.tracks.len(), 2);
        assert_eq!(handover.tracks[0].song_id.as_deref(), Some("nd-1"));
        assert_eq!(handover.tracks[0].gain.as_ref().unwrap().album, Some(-4.0));
        assert_eq!(handover.tracks[1].song_id, None);
        assert_eq!(handover.time, 12.5);
        assert_eq!(parse_handover("null"), None);
        assert_eq!(
            parse_handover(
                r#"{"tracks":[],"position":0,"time":0,"playing":true,"volume":1,"muted":false,"levelling":true,"repeat":"off"}"#
            ),
            None
        );
    }

    #[test]
    fn levels_like_the_web_player() {
        let gain = Gain {
            track: Some(-6.0),
            album: Some(-3.0),
            track_peak: None,
            album_peak: None,
        };
        assert_eq!(gain_level(None, false), 1.0);
        assert_eq!(gain_level(Some(&gain), false), 0.501);
        assert_eq!(gain_level(Some(&gain), true), 0.708);
        let loud = Gain {
            track: Some(-1.0),
            track_peak: Some(1.25),
            ..Gain::default()
        };
        assert_eq!(gain_level(Some(&loud), false), 0.8);
        let quiet = Gain {
            track: Some(4.0),
            ..Gain::default()
        };
        assert_eq!(gain_level(Some(&quiet), false), 1.0);
    }

    #[test]
    fn knows_when_an_album_is_playing() {
        let tracks = vec![
            track("One", Some("X")),
            track("Two", Some("X")),
            track("Three", Some("Y")),
        ];
        assert!(playing_an_album(&tracks, 0));
        assert!(playing_an_album(&tracks, 1));
        assert!(!playing_an_album(&tracks, 2));
    }

    #[test]
    fn moves_through_the_queue_like_the_web_player() {
        assert_eq!(next_index(0, 3, "off"), Some(1));
        assert_eq!(next_index(2, 3, "off"), None);
        assert_eq!(next_index(2, 3, "all"), Some(0));
        assert_eq!(next_index(1, 3, "one"), Some(1));
        assert_eq!(next_index(0, 1, "all"), None);
        assert_eq!(next_repeat("off"), "all");
        assert_eq!(next_repeat("all"), "one");
        assert_eq!(next_repeat("one"), "off");
    }

    #[test]
    fn counts_a_play_like_the_web_player() {
        assert!(!counts_as_play(100.0, 25.0));
        assert!(counts_as_play(60.0, 120.0));
        assert!(!counts_as_play(59.0, 120.0));
        assert!(counts_as_play(240.0, 1200.0));
        assert_eq!(heard_step(None, 5.0), 0.0);
        assert_eq!(heard_step(Some(5.0), 5.5), 0.5);
        assert_eq!(heard_step(Some(5.0), 40.0), 0.0);
        assert_eq!(heard_step(Some(5.0), 4.0), 0.0);
    }

    #[test]
    fn streams_through_the_relay_with_full_songs_first() {
        let server = Url::parse("https://music.example/").unwrap();
        let relay = Relay::start(server.clone(), "t".into()).unwrap();
        let full = track("One", None);
        let address = source_for(&relay, &full).unwrap();
        assert_eq!(address.host_str(), Some("127.0.0.1"));
        assert_eq!(address.path(), "/s/full-song/nd-One");
        let preview = Track {
            song_id: None,
            track_id: "band-a b".into(),
            ..full.clone()
        };
        assert_eq!(
            source_for(&relay, &preview).unwrap().path(),
            "/s/listen/band-a%20b"
        );
        assert_eq!(
            cover_for(&server, &full).as_deref(),
            Some("https://music.example/api/navidrome/cover/al-1")
        );
    }

    #[test]
    fn hands_the_place_back_to_the_page_once() {
        let script = handback_script(
            &Resume {
                position: 2,
                time: 61.37,
                playing: true,
            },
            42,
        );
        assert!(script.contains("queue.position = 2;"));
        assert!(script.contains("queue.time = 61.4;"));
        assert!(script.contains(r#"=== "42""#));
        assert!(script.contains("if (true)"));
    }

    #[test]
    fn maps_media_controls_to_the_player() {
        assert!(matches!(command_for("toggle", 0.0), Some(Command::Toggle)));
        assert!(matches!(command_for("seek", 5.0), Some(Command::SeekBy(s)) if s == 5.0));
        assert!(matches!(command_for("position", 30.0), Some(Command::SeekTo(s)) if s == 30.0));
        assert!(command_for("shuffle", 0.0).is_none());
    }
}
