use crate::media::{Preview, UpNext};
use crate::relay::Relay;
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;

const TICK: Duration = Duration::from_millis(250);
const FADE_TICK: Duration = Duration::from_millis(20);
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
    #[serde(default)]
    pub duration: Option<f64>,
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
    #[serde(default)]
    pub crossfade: f64,
}

const SEEK_ARRIVED: f64 = 1.0;
const SEEK_GIVE_UP: f64 = 4.0;
const NOT_STARTED: f64 = 0.05;

pub fn shown_position(target: Option<(f64, f64)>, actual: Option<f64>) -> (Option<f64>, bool) {
    let loading = matches!(target, Some((wanted, _)) if wanted >= SEEK_ARRIVED)
        && actual.is_some_and(|now| now < NOT_STARTED);
    let actual = if loading { None } else { actual };
    match (target, actual) {
        (Some((wanted, waited)), Some(now)) => {
            if (now - wanted).abs() < SEEK_ARRIVED || waited >= SEEK_GIVE_UP {
                (Some(now), false)
            } else {
                (Some(wanted), true)
            }
        }
        (Some((wanted, _)), None) => (Some(wanted), true),
        (None, actual) => (actual, false),
    }
}

pub fn crossfade_due(crossfade: f64, length: f64, position: f64) -> bool {
    crossfade > 0.0 && length >= crossfade * 3.0 && length - position <= crossfade
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
    SetRepeat(String),
    Volume(f64),
    Enqueue(Box<Track>, bool),
    Album(String, String),
    Jump(usize),
    Settings {
        volume: f64,
        muted: bool,
        levelling: bool,
        crossfade: f64,
    },
    FadeTo(f64, Duration),
    Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub track: Track,
    pub playing: bool,
    pub time: f64,
    pub duration: Option<f64>,
    pub volume: f64,
    pub repeat: String,
    pub up_next: Vec<Track>,
}

pub fn insert_at(at: usize, length: usize, next: bool) -> usize {
    if next {
        (at + 1).min(length)
    } else {
        length
    }
}

pub fn shifted(coming: Option<usize>, inserted: usize) -> Option<usize> {
    coming.map(|index| if index >= inserted { index + 1 } else { index })
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
    snapshot: Option<Snapshot>,
    resume: Resume,
    clock: Option<Clock>,
}

#[derive(Clone, Copy, Debug)]
struct Clock {
    at: usize,
    position: f64,
    measured: Instant,
    moving: bool,
}

pub fn clock_now(at: usize, position: f64, elapsed: f64, moving: bool) -> (usize, f64) {
    (at, if moving { position + elapsed } else { position })
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

    pub fn snapshot(&self) -> Option<Snapshot> {
        self.shown.lock().expect("shown lock").snapshot.clone()
    }

    pub fn resume(&self) -> Resume {
        self.shown.lock().expect("shown lock").resume.clone()
    }

    pub fn now(&self) -> Option<(usize, f64, bool)> {
        let clock = self.shown.lock().expect("shown lock").clock?;
        let (at, position) = clock_now(
            clock.at,
            clock.position,
            clock.measured.elapsed().as_secs_f64(),
            clock.moving,
        );
        Some((at, position, clock.moving))
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

pub type Report = Box<dyn Fn(&'static str, Track) + Send>;

struct Outgoing {
    playbin: gst::Element,
    from: f64,
    began: Instant,
    length: Duration,
}

fn make_playbin(
    queue: &Arc<Mutex<Queue>>,
    relay: &Relay,
) -> Result<(gst::Element, Arc<AtomicBool>), String> {
    let playbin = gst::ElementFactory::make("playbin")
        .build()
        .map_err(|_| "GStreamer's playbin is missing".to_string())?;
    if let Some(sink) = audio_sink() {
        playbin.set_property("audio-sink", &sink);
    }
    if let Ok(fake) = gst::ElementFactory::make("fakesink").build() {
        playbin.set_property("video-sink", &fake);
    }
    let active = Arc::new(AtomicBool::new(true));
    let still = active.clone();
    let next_queue = queue.clone();
    let next_relay = relay.clone();
    playbin.connect("about-to-finish", false, move |values| {
        if !still.load(Ordering::Relaxed) {
            return None;
        }
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
    Ok((playbin, active))
}

struct Engine {
    playbin: gst::Element,
    seek_target: std::cell::Cell<Option<(f64, Instant)>>,
    last_shown: std::cell::Cell<f64>,
    active: Arc<AtomicBool>,
    outgoing: Option<Outgoing>,
    crossfade: f64,
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
    fade_gain: f64,
    fade: Option<(f64, f64, Instant, Duration)>,
    previous_position: Option<f64>,
}

impl Engine {
    fn crossfade_soon(&mut self) {
        if self.crossfade <= 0.0
            || !self.want_playing
            || self.released
            || self.outgoing.is_some()
            || self.seek_when_ready.is_some()
        {
            return;
        }
        let Some(length) = self.stream_length() else {
            return;
        };
        if !crossfade_due(self.crossfade, length, self.position()) {
            return;
        }
        let next = {
            let queue = self.queue.lock().expect("queue lock");
            if queue.coming.is_some() || queue.repeat == "one" {
                return;
            }
            let Some(next) = next_index(queue.at, queue.tracks.len(), &queue.repeat) else {
                return;
            };
            if next == queue.at || same_album(queue.tracks.get(queue.at), queue.tracks.get(next)) {
                return;
            }
            next
        };
        let Ok((fresh, active)) = make_playbin(&self.queue, &self.relay) else {
            return;
        };
        self.active.store(false, Ordering::Relaxed);
        let old = std::mem::replace(&mut self.playbin, fresh);
        self.active = active;
        let from = old.property::<f64>("volume");
        let length = Duration::from_secs_f64(self.crossfade);
        self.outgoing = Some(Outgoing {
            playbin: old,
            from,
            began: Instant::now(),
            length,
        });
        self.fade_gain = 0.0;
        self.fade = Some((0.0, 1.0, Instant::now(), length));
        self.load(next, 0.0);
    }

    fn step_outgoing(&mut self) {
        let Some(outgoing) = &self.outgoing else {
            return;
        };
        let done = (outgoing.began.elapsed().as_secs_f64()
            / outgoing.length.as_secs_f64().max(0.001))
        .min(1.0);
        outgoing
            .playbin
            .set_property("volume", (outgoing.from * (1.0 - done)).clamp(0.0, 1.0));
        if done >= 1.0 {
            self.drop_outgoing();
        }
    }

    fn drop_outgoing(&mut self) {
        if let Some(outgoing) = self.outgoing.take() {
            let _ = outgoing.playbin.set_state(gst::State::Null);
        }
    }

    fn step_fade(&mut self) {
        let Some((from, to, began, length)) = self.fade else {
            return;
        };
        let done = (began.elapsed().as_secs_f64() / length.as_secs_f64().max(0.001)).min(1.0);
        self.fade_gain = from + (to - from) * done;
        if done >= 1.0 {
            self.fade = None;
        }
        self.apply_volume();
    }

    fn measure(&mut self) {
        let position = self.position();
        let moving = self.want_playing
            && !self.released
            && self.seek_when_ready.is_none()
            && self
                .previous_position
                .is_some_and(|before| position > before);
        self.previous_position = Some(position);
        let at = self.queue.lock().expect("queue lock").at;
        self.shown.lock().expect("shown lock").clock = Some(Clock {
            at,
            position,
            measured: Instant::now(),
            moving,
        });
    }

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
        self.playbin.set_property(
            "volume",
            (self.volume * level * self.fade_gain).clamp(0.0, 1.0),
        );
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
        self.seek_target.set(None);
        self.last_shown.set(time.max(0.0));
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
        let actual = self
            .playbin
            .query_position::<gst::ClockTime>()
            .map(|time| time.nseconds() as f64 / 1e9);
        let target = self
            .seek_target
            .get()
            .map(|(wanted, asked)| (wanted, asked.elapsed().as_secs_f64()));
        let (shown, keep) = shown_position(target, actual);
        if !keep {
            self.seek_target.set(None);
        }
        let shown = shown
            .or(self.seek_when_ready)
            .unwrap_or(self.last_shown.get());
        self.last_shown.set(shown);
        shown
    }

    fn known_length(&self) -> Option<f64> {
        self.stream_length()
            .or_else(|| self.track().and_then(|track| track.duration))
    }

    fn stream_length(&self) -> Option<f64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|time| time.nseconds() as f64 / 1e9)
            .filter(|seconds| *seconds > 0.0)
    }

    fn seek(&mut self, seconds: f64) {
        let seconds = match self.known_length() {
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
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, target)
            .is_err()
        {
            self.seek_when_ready = Some(seconds);
        }
        self.seek_target.set(Some((seconds, Instant::now())));
        self.last = None;
        self.previous_position = None;
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
        if !matches!(
            command,
            Command::Volume(_)
                | Command::Settings { .. }
                | Command::FadeTo(..)
                | Command::SetRepeat(_)
                | Command::Album(..)
        ) {
            self.drop_outgoing();
            if self.fade.is_some() {
                self.fade = None;
                self.fade_gain = 1.0;
                self.apply_volume();
            }
        }
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
            Command::SetRepeat(mode) => {
                self.queue.lock().expect("queue lock").repeat = mode;
            }
            Command::Volume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.muted = false;
                self.apply_volume();
            }
            Command::Album(track_id, album) => {
                let mut queue = self.queue.lock().expect("queue lock");
                for track in queue.tracks.iter_mut() {
                    if track.track_id == track_id && track.album.is_none() {
                        track.album = Some(album.clone());
                    }
                }
            }
            Command::Enqueue(track, next) => {
                let mut queue = self.queue.lock().expect("queue lock");
                let at = insert_at(queue.at, queue.tracks.len(), next);
                queue.tracks.insert(at, *track);
                queue.coming = shifted(queue.coming, at);
            }
            Command::Settings {
                volume,
                muted,
                levelling,
                crossfade,
            } => {
                self.volume = volume.clamp(0.0, 1.0);
                self.muted = muted;
                self.levelling = levelling;
                self.crossfade = crossfade.max(0.0);
                self.apply_volume();
            }
            Command::Jump(ahead) => {
                let (at, length) = {
                    let queue = self.queue.lock().expect("queue lock");
                    (queue.at, queue.tracks.len())
                };
                if ahead > 0 && at + ahead < length {
                    self.load(at + ahead, 0.0);
                }
            }
            Command::FadeTo(to, length) => {
                self.fade = Some((self.fade_gain, to.clamp(0.0, 1.0), Instant::now(), length));
                self.step_fade();
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
                        self.seek_target.set(None);
                        self.last_shown.set(0.0);
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
        if self.released || self.stream_length().is_none() {
            return;
        }
        let target = gst::ClockTime::from_nseconds((seconds * 1e9) as u64);
        if self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, target)
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
        if track.song_id.is_some() && counts_as_play(self.heard, self.known_length().unwrap_or(0.0))
        {
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
            length: self.stream_length().or(track.duration),
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
        let snapshot = Snapshot {
            track: track.clone(),
            playing: self.want_playing && !self.released,
            time: position,
            duration: self.stream_length().or(track.duration),
            volume: self.volume,
            repeat: queue.repeat.clone(),
            up_next: queue
                .tracks
                .iter()
                .skip(queue.at + 1)
                .take(10)
                .cloned()
                .collect(),
        };
        drop(queue);
        let mut shown = self.shown.lock().expect("shown lock");
        shown.preview = Some(preview);
        shown.snapshot = Some(snapshot);
        shown.resume = resume;
    }
}

fn audio_sink() -> Option<gst::Element> {
    let description = std::env::var("SOUNDSCAPE_AUDIO_SINK").ok()?;
    gst::parse::launch(&description).ok()
}

pub fn start(
    server: Url,
    token: String,
    handover: Handover,
    report: Report,
    silent: bool,
) -> Result<Player, String> {
    gst::init().map_err(|error| error.to_string())?;
    let relay = Relay::start(server.clone(), token)?;

    let queue = Arc::new(Mutex::new(Queue {
        tracks: handover.tracks.clone(),
        at: handover.position,
        repeat: handover.repeat.clone(),
        coming: None,
    }));
    let (playbin, active) = make_playbin(&queue, &relay)?;

    let shown = Arc::new(Mutex::new(Shown {
        preview: None,
        snapshot: None,
        resume: Resume {
            position: handover.position,
            time: handover.time,
            playing: handover.playing,
        },
        clock: None,
    }));
    let (commands, receive) = channel::<Command>();
    let mut engine = Engine {
        playbin,
        active,
        outgoing: None,
        crossfade: handover.crossfade.max(0.0),
        queue,
        shown: shown.clone(),
        server,
        relay,
        volume: handover.volume.clamp(0.0, 1.0),
        muted: handover.muted,
        levelling: handover.levelling,
        want_playing: handover.playing,
        seek_when_ready: None,
        seek_target: std::cell::Cell::new(None),
        last_shown: std::cell::Cell::new(handover.time.max(0.0)),
        released: false,
        resume_time: 0.0,
        paused_since: (!handover.playing).then(Instant::now),
        heard: 0.0,
        last: None,
        counted: false,
        failures: 0,
        report,
        fade_gain: if silent { 0.0 } else { 1.0 },
        fade: None,
        previous_position: None,
    };
    std::thread::Builder::new()
        .name("soundscape-player".into())
        .spawn(move || {
            engine.load(handover.position, handover.time);
            loop {
                let wait = if engine.fade.is_some() || engine.outgoing.is_some() {
                    FADE_TICK
                } else {
                    TICK
                };
                match receive.recv_timeout(wait) {
                    Ok(command) => {
                        if !engine.handle(command) {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        engine.drop_outgoing();
                        let _ = engine.playbin.set_state(gst::State::Null);
                        break;
                    }
                }
                engine.step_fade();
                engine.step_outgoing();
                engine.crossfade_soon();
                engine.messages();
                engine.seek_when_ready();
                engine.measure();
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
            duration: Some(60.0),
        }
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
        let parts: Vec<&str> = address.path().split('/').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!((parts[1], parts[3], parts[4]), ("s", "full-song", "nd-One"));
        assert_eq!(parts[2].len(), 32);
        let preview = Track {
            song_id: None,
            track_id: "band-a b".into(),
            ..full.clone()
        };
        assert!(source_for(&relay, &preview)
            .unwrap()
            .path()
            .ends_with("/listen/band-a%20b"));
        assert_eq!(
            cover_for(&server, &full).as_deref(),
            Some("https://music.example/api/navidrome/cover/al-1")
        );
    }

    #[test]
    fn keeps_time_between_measurements() {
        assert_eq!(clock_now(2, 10.0, 0.25, true), (2, 10.25));
        assert_eq!(clock_now(2, 10.0, 0.25, false), (2, 10.0));
    }

    #[test]
    fn maps_media_controls_to_the_player() {
        assert!(matches!(command_for("toggle", 0.0), Some(Command::Toggle)));
        assert!(matches!(command_for("seek", 5.0), Some(Command::SeekBy(s)) if s == 5.0));
        assert!(matches!(command_for("position", 30.0), Some(Command::SeekTo(s)) if s == 30.0));
        assert!(command_for("shuffle", 0.0).is_none());
    }

    #[test]
    fn adds_songs_where_the_page_asks() {
        assert_eq!(insert_at(2, 5, true), 3);
        assert_eq!(insert_at(2, 5, false), 5);
        assert_eq!(insert_at(4, 5, true), 5);
        assert_eq!(shifted(Some(3), 3), Some(4));
        assert_eq!(shifted(Some(2), 3), Some(2));
        assert_eq!(shifted(None, 0), None);
    }

    #[test]
    fn crossfades_only_near_the_end_of_long_enough_songs() {
        assert!(crossfade_due(6.0, 200.0, 195.0));
        assert!(!crossfade_due(6.0, 200.0, 190.0));
        assert!(!crossfade_due(0.0, 200.0, 199.0));
        assert!(!crossfade_due(6.0, 15.0, 14.0));
        assert!(crossfade_due(2.0, 6.0, 4.5));
    }

    #[test]
    fn shows_where_a_seek_is_going_until_it_gets_there() {
        assert_eq!(
            shown_position(Some((120.0, 0.1)), Some(30.0)),
            (Some(120.0), true)
        );
        assert_eq!(
            shown_position(Some((120.0, 0.3)), Some(120.4)),
            (Some(120.4), false)
        );
        assert_eq!(
            shown_position(Some((120.0, 5.0)), Some(30.0)),
            (Some(30.0), false)
        );
        assert_eq!(
            shown_position(Some((120.0, 0.2)), None),
            (Some(120.0), true)
        );
        assert_eq!(
            shown_position(Some((120.0, 9.0)), None),
            (Some(120.0), true)
        );
        assert_eq!(
            shown_position(Some((120.0, 9.0)), Some(0.0)),
            (Some(120.0), true)
        );
        assert_eq!(
            shown_position(Some((0.4, 0.2)), Some(0.0)),
            (Some(0.0), false)
        );
        assert_eq!(shown_position(None, Some(42.0)), (Some(42.0), false));
    }
}
