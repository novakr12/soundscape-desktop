use crate::media::{Preview, UpNext};
use crate::relay::Relay;
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use url::Url;

const TICK: Duration = Duration::from_millis(250);
pub const SPECTRUM_TICK: Duration = Duration::from_millis(15);
pub const SPECTRUM_EVERY: Duration = Duration::from_millis(30);
const FADE_TICK: Duration = Duration::from_millis(20);
const SLEEP_FADE: Duration = Duration::from_secs(10);
const SPECTRUM_BAR_CHOICES: [usize; 4] = [4, 6, 8, 12];
const SPECTRUM_BARS: usize = 8;
pub const SPECTRUM_REST: f32 = 0.15;
const SPECTRUM_RAW_BANDS: u32 = 128;
const SPECTRUM_THRESHOLD_DB: i32 = -70;
const SPECTRUM_LEAD_LIMIT: gst::ClockTime = gst::ClockTime::from_seconds(5);
const RESTART_AFTER_SECONDS: f64 = 3.0;
const SHORTEST_SONG_SECONDS: f64 = 30.0;
const LONGEST_WAIT_SECONDS: f64 = 240.0;
const LONGEST_STEP_SECONDS: f64 = 2.0;
const RELEASE_AFTER: Duration = Duration::from_secs(2 * 60);
const MOST_FAILURES_IN_A_ROW: u32 = 5;

fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Gain {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<f64>,
    #[serde(default, rename = "trackPeak", skip_serializing_if = "Option::is_none")]
    pub track_peak: Option<f64>,
    #[serde(default, rename = "albumPeak", skip_serializing_if = "Option::is_none")]
    pub album_peak: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Track {
    #[serde(rename = "trackId")]
    pub track_id: String,
    #[serde(default, rename = "songId", skip_serializing_if = "Option::is_none")]
    pub song_id: Option<String>,
    pub title: String,
    pub artist: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<Gain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

fn default_repeat() -> String {
    "off".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SavedQueue {
    pub server: String,
    pub tracks: Vec<Track>,
    pub position: usize,
    #[serde(default)]
    pub time: f64,
    #[serde(default = "default_repeat")]
    pub repeat: String,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unshuffled: Option<Vec<(String, Option<String>)>>,
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
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub restored: bool,
    #[serde(default)]
    pub unshuffled: Option<Vec<(String, Option<String>)>>,
    #[serde(skip)]
    pub visualizer: Visualizer,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    Wave,
    Dots,
    #[default]
    #[serde(other)]
    Bars,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Bounce {
    Gentle,
    Normal,
    Extreme,
    #[default]
    #[serde(other)]
    Lively,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visualizer {
    pub on: bool,
    pub style: Style,
    pub bounce: Bounce,
    pub bars: usize,
}

impl Visualizer {
    pub const DEFAULT: Visualizer = Visualizer {
        on: true,
        style: Style::Bars,
        bounce: Bounce::Lively,
        bars: SPECTRUM_BARS,
    };

    pub fn new(on: bool, style: Style, bounce: Bounce, bars: usize) -> Visualizer {
        Visualizer {
            on,
            style,
            bounce,
            bars: if SPECTRUM_BAR_CHOICES.contains(&bars) {
                bars
            } else {
                SPECTRUM_BARS
            },
        }
    }
}

impl Default for Visualizer {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn band_edges(bars: usize) -> &'static [usize] {
    match bars {
        4 => &[0, 2, 7, 25, 128],
        6 => &[0, 2, 4, 9, 20, 48, 128],
        12 => &[0, 1, 2, 3, 5, 7, 10, 14, 21, 32, 48, 76, 128],
        _ => &[0, 1, 2, 4, 7, 13, 25, 52, 128],
    }
}

fn bounce_profile(bounce: Bounce) -> (f32, f32, f32) {
    match bounce {
        Bounce::Gentle => (1.9, 3.0, 0.80),
        Bounce::Normal => (1.65, 1.0, 0.73),
        Bounce::Lively => (1.45, -0.5, 0.63),
        Bounce::Extreme => (1.2, -3.0, 0.50),
    }
}

pub fn normalize_spectrum_db(
    magnitudes: &[f32],
    previous: Option<&[f32]>,
    bars: usize,
    bounce: Bounce,
) -> Vec<f32> {
    let edges = band_edges(bars);
    let band_count = edges.len() - 1;
    let mut out = vec![SPECTRUM_REST; band_count];
    if magnitudes.is_empty() {
        return out;
    }
    let (exponent, ceil_offset, decay) = bounce_profile(bounce);
    let count = magnitudes.len();
    for (i, slot) in out.iter_mut().enumerate() {
        let (start, end) = if count >= 16 {
            let s = (edges[i] * count / 128).min(count - 1);
            let e = (edges[i + 1] * count / 128).max(s + 1).min(count);
            (s, e)
        } else {
            let s = (i * count / band_count).min(count - 1);
            let e = ((i + 1) * count / band_count).max(s + 1).min(count);
            (s, e)
        };
        let mut peak_db = SPECTRUM_THRESHOLD_DB as f32;
        let mut sum_linear = 0.0f32;
        for &db in &magnitudes[start..end] {
            if db > peak_db {
                peak_db = db;
            }
            sum_linear += 10.0f32.powf(db / 20.0);
        }
        let avg_db = 20.0 * (sum_linear / (end - start) as f32).max(1e-6).log10();
        let effective_db = peak_db * 0.75 + avg_db * 0.25;
        let share = if band_count > 1 {
            i as f32 / (band_count - 1) as f32
        } else {
            0.0
        };
        let floor = -46.0 - share * 15.0;
        let ceil = -10.0 - share * 17.0 + ceil_offset;
        let norm = ((effective_db - floor) / (ceil - floor)).clamp(0.0, 1.0);
        let curved = norm.powf(exponent);
        let target = (0.15 + 0.85 * curved).clamp(0.15, 1.0);
        let prev = previous.and_then(|p| p.get(i).copied()).unwrap_or(0.15);
        let next = if target >= prev {
            target
        } else {
            (prev * decay + target * (1.0 - decay)).max(0.15)
        };
        *slot = if next < 0.165 { 0.15 } else { next };
    }
    out
}

fn read_spectrum(structure: &gst::StructureRef) -> Option<(gst::ClockTime, Vec<f32>)> {
    if structure.name() != "spectrum" {
        return None;
    }
    let at = structure.get::<gst::ClockTime>("running-time").ok()?;
    Some((at, read_spectrum_magnitudes(structure)?))
}

fn due_spectrum(
    ahead: &mut VecDeque<(gst::ClockTime, Vec<f32>)>,
    now: gst::ClockTime,
) -> Option<Vec<f32>> {
    ahead.retain(|(at, _)| *at <= now + SPECTRUM_LEAD_LIMIT);
    let mut due = None;
    while ahead.front().is_some_and(|(at, _)| *at <= now) {
        due = ahead.pop_front().map(|(_, mags)| mags);
    }
    due
}

pub fn read_spectrum_magnitudes(structure: &gst::StructureRef) -> Option<Vec<f32>> {
    let value = structure.value("magnitude").ok()?;
    if let Ok(list) = value.get::<gst::List>() {
        let mags: Vec<f32> = list
            .iter()
            .filter_map(|item| {
                item.get::<f32>()
                    .ok()
                    .or_else(|| item.get::<f64>().ok().map(|v| v as f32))
            })
            .collect();
        if !mags.is_empty() {
            return Some(mags);
        }
    }
    if let Ok(array) = value.get::<gst::Array>() {
        let mags: Vec<f32> = array
            .iter()
            .filter_map(|item| {
                item.get::<f32>()
                    .ok()
                    .or_else(|| item.get::<f64>().ok().map(|v| v as f32))
            })
            .collect();
        if !mags.is_empty() {
            return Some(mags);
        }
    }
    None
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

/// Map a uniform UI slider position in `0..=1` onto a cubic perceptual loudness curve.
pub fn volume_gain(volume: f64) -> f64 {
    if !volume.is_finite() {
        return 0.0;
    }
    volume.clamp(0.0, 1.0).powi(3)
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SleepRequest {
    AfterSong,
    Seconds(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sleep {
    AfterSong,
    At(Instant),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SleepSnapshot {
    AfterSong,
    Remaining(f64),
}

fn sleep_snapshot(sleep: Option<Sleep>, now: Instant) -> Option<SleepSnapshot> {
    match sleep {
        Some(Sleep::AfterSong) => Some(SleepSnapshot::AfterSong),
        Some(Sleep::At(deadline)) => Some(SleepSnapshot::Remaining(
            deadline.saturating_duration_since(now).as_secs_f64(),
        )),
        None => None,
    }
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
    Shuffle(Option<bool>),
    Volume(f64),
    Sleep(Option<SleepRequest>),
    Enqueue(Box<Track>, bool),
    Album(String, String),
    Jump(usize),
    Remove(usize),
    Move(usize, usize),
    Settings {
        volume: f64,
        muted: bool,
        levelling: bool,
        crossfade: f64,
        visualizer: Visualizer,
    },
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
    pub shuffle: bool,
    pub sleep: Option<SleepSnapshot>,
    pub up_next: Vec<Track>,
}

pub fn insert_at(at: usize, length: usize, next: bool) -> usize {
    if next {
        (at + 1).min(length)
    } else {
        length
    }
}

pub fn shuffle_tracks(tracks: &mut [Track], seed: u64) {
    let mut state = seed | 1;
    for i in (1..tracks.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        tracks.swap(i, j);
    }
}

pub fn restore_order(original: &[(String, Option<String>)], upcoming: Vec<Track>) -> Vec<Track> {
    let mut left: Vec<Option<Track>> = upcoming.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(left.len());
    for (track_id, song_id) in original {
        let found = left.iter_mut().find(|slot| {
            slot.as_ref()
                .is_some_and(|track| track.track_id == *track_id && track.song_id == *song_id)
        });
        if let Some(track) = found.and_then(Option::take) {
            out.push(track);
        }
    }
    out.extend(left.into_iter().flatten());
    out
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| since.as_nanos() as u64)
}

pub fn shifted(coming: Option<usize>, inserted: usize) -> Option<usize> {
    coming.map(|index| if index >= inserted { index + 1 } else { index })
}

pub fn remove_upcoming(
    tracks: &mut Vec<Track>,
    at: usize,
    ahead: usize,
    coming: Option<usize>,
    unshuffled: &mut Option<Vec<(String, Option<String>)>>,
) -> Option<(Option<usize>, bool)> {
    if ahead == 0 || at + ahead >= tracks.len() {
        return None;
    }
    let target = at + ahead;
    let removed = tracks.remove(target);
    if let Some(original) = unshuffled.as_mut() {
        if let Some(found) = original
            .iter()
            .position(|(id, song_id)| *id == removed.track_id && *song_id == removed.song_id)
        {
            original.remove(found);
        }
    }
    Some(match coming {
        Some(c) if c == target => (None, true),
        Some(c) if c > target => (Some(c - 1), false),
        other => (other, false),
    })
}

pub fn move_upcoming(
    tracks: &mut Vec<Track>,
    at: usize,
    from: usize,
    to: usize,
    coming: Option<usize>,
    repeat: &str,
) -> Option<(Option<usize>, bool)> {
    if from == 0 || to == 0 || from == to || at + from >= tracks.len() || at + to >= tracks.len() {
        return None;
    }
    let src = at + from;
    let dst = at + to;
    let item = tracks.remove(src);
    tracks.insert(dst, item);
    let shifted_coming = coming.map(|c| {
        if c == src {
            dst
        } else if src < dst && src < c && c <= dst {
            c - 1
        } else if dst < src && dst <= c && c < src {
            c + 1
        } else {
            c
        }
    });
    if coming.is_some() && next_index(at, tracks.len(), repeat) != shifted_coming {
        Some((None, true))
    } else {
        Some((shifted_coming, false))
    }
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
        "shuffle" => Command::Shuffle(None),
        _ => return None,
    })
}

struct Queue {
    tracks: Vec<Track>,
    at: usize,
    repeat: String,
    coming: Option<usize>,
    unshuffled: Option<Vec<(String, Option<String>)>>,
    sleep_after_song: bool,
}

impl Queue {
    fn shuffle_from(&self) -> usize {
        self.coming.unwrap_or(self.at).max(self.at) + 1
    }

    fn set_shuffle(&mut self, on: bool) {
        let from = self.shuffle_from().min(self.tracks.len());
        match (on, self.unshuffled.take()) {
            (true, None) => {
                self.unshuffled = Some(
                    self.tracks[from..]
                        .iter()
                        .map(|track| (track.track_id.clone(), track.song_id.clone()))
                        .collect(),
                );
                shuffle_tracks(&mut self.tracks[from..], seed());
            }
            (false, Some(original)) => {
                let upcoming = self.tracks.split_off(from);
                self.tracks.extend(restore_order(&original, upcoming));
            }
            (_, kept) => self.unshuffled = kept,
        }
    }
}

struct Shown {
    preview: Option<Preview>,
    snapshot: Option<Snapshot>,
    spectrum: Option<Vec<f32>>,
}

pub struct Player {
    commands: Sender<Command>,
    shown: Arc<Mutex<Shown>>,
    queue: Arc<Mutex<Queue>>,
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

    pub fn spectrum(&self) -> Option<Vec<f32>> {
        self.shown.lock().expect("shown lock").spectrum.clone()
    }

    pub fn queue_tracks(&self) -> Vec<Track> {
        self.queue.lock().expect("queue lock").tracks.clone()
    }

    pub fn saved_state(&self, server: &str) -> Option<SavedQueue> {
        let time = self.snapshot().map_or(0.0, |snapshot| snapshot.time);
        let queue = self.queue.lock().expect("queue lock");
        if queue.tracks.is_empty() || queue.at >= queue.tracks.len() {
            return None;
        }
        Some(SavedQueue {
            server: server.to_string(),
            tracks: queue.tracks.clone(),
            position: queue.at,
            time: if time.is_finite() && time > 0.0 {
                time
            } else {
                0.0
            },
            repeat: queue.repeat.clone(),
            shuffle: queue.unshuffled.is_some(),
            unshuffled: queue.unshuffled.clone(),
        })
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
    visualizer: bool,
) -> Result<(gst::Element, Arc<AtomicBool>, Option<gst::Element>), String> {
    let playbin = gst::ElementFactory::make("playbin")
        .build()
        .map_err(|_| "GStreamer's playbin is missing".to_string())?;
    playbin.set_property_from_str("flags", "audio+soft-volume");
    playbin.set_property("buffer-size", 2i32 * 1024 * 1024);
    playbin.set_property("buffer-duration", 10_000_000_000i64);
    if let Some(sink) = audio_sink() {
        playbin.set_property("audio-sink", &sink);
    }
    let spectrum = gst::ElementFactory::make("spectrum").build().ok();
    if let Some(ref spec) = spectrum {
        spec.set_property("bands", SPECTRUM_RAW_BANDS);
        spec.set_property("threshold", SPECTRUM_THRESHOLD_DB);
        spec.set_property("interval", SPECTRUM_EVERY.as_nanos() as u64);
        spec.set_property("post-messages", visualizer);
        playbin.set_property("audio-filter", spec);
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
        if queue.sleep_after_song {
            return None;
        }
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
    Ok((playbin, active, spectrum))
}

struct Engine {
    playbin: gst::Element,
    spectrum_element: Option<gst::Element>,
    visualizer: Visualizer,
    heard_ahead: VecDeque<(gst::ClockTime, Vec<f32>)>,
    levels: Vec<f32>,
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
    sleep: Option<Sleep>,
    sleep_fading: bool,
}

impl Engine {
    fn crossfade_soon(&mut self) {
        if self.crossfade <= 0.0
            || !self.want_playing
            || self.released
            || self.outgoing.is_some()
            || self.seek_when_ready.is_some()
            || matches!(self.sleep, Some(Sleep::AfterSong))
            || self.sleep_fading
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
        let Ok((fresh, active, spectrum)) =
            make_playbin(&self.queue, &self.relay, self.visualizer.on)
        else {
            return;
        };
        self.active.store(false, Ordering::Relaxed);
        let old = std::mem::replace(&mut self.playbin, fresh);
        self.spectrum_element = spectrum;
        self.heard_ahead.clear();
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

    fn step_sleep(&mut self) {
        let Some(Sleep::At(deadline)) = self.sleep else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        self.sleep = None;
        if self.want_playing && !self.released {
            self.drop_outgoing();
            self.fade = Some((self.fade_gain, 0.0, Instant::now(), SLEEP_FADE));
            self.sleep_fading = true;
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
            if self.sleep_fading {
                self.sleep_fading = false;
                self.fade_gain = 0.0;
                self.apply_volume();
                self.set_playing(false);
                let _ = self.playbin.state(Some(gst::ClockTime::from_mseconds(50)));
                self.fade_gain = 1.0;
            }
        }
        self.apply_volume();
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
            (volume_gain(self.volume) * level * self.fade_gain).clamp(0.0, 1.0),
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
        self.heard_ahead.clear();
        self.playbin.set_property("uri", uri.as_str());
        self.seek_when_ready = (time > 0.5).then_some(time);
        self.released = false;
        let state = if self.want_playing {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        self.apply_volume();
        let _ = self.playbin.set_state(state);
        self.started();
    }

    fn reload_current_in_place(&mut self) {
        if self.released {
            return;
        }
        let pos = self.position();
        let uri = {
            let mut queue = self.queue.lock().expect("queue lock");
            queue.coming = None;
            queue
                .tracks
                .get(queue.at)
                .and_then(|track| source_for(&self.relay, track))
        };
        let Some(uri) = uri else {
            return;
        };
        let _ = self.playbin.set_state(gst::State::Null);
        self.heard_ahead.clear();
        self.playbin.set_property("uri", uri.as_str());
        self.seek_when_ready = (pos > 0.5).then_some(pos);
        self.seek_target.set(None);
        self.last_shown.set(pos);
        let state = if self.want_playing {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        self.apply_volume();
        let _ = self.playbin.set_state(state);
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
        self.heard_ahead.clear();
        self.seek_target.set(Some((seconds, Instant::now())));
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
        if !matches!(
            command,
            Command::Volume(_)
                | Command::Settings { .. }
                | Command::SetRepeat(_)
                | Command::Shuffle(_)
                | Command::Sleep(_)
                | Command::Album(..)
                | Command::Remove(_)
                | Command::Move(..)
        ) {
            self.drop_outgoing();
            if self.fade.is_some() {
                self.fade = None;
                self.sleep_fading = false;
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
            Command::Shuffle(on) => {
                let mut queue = self.queue.lock().expect("queue lock");
                let on = on.unwrap_or(queue.unshuffled.is_none());
                queue.set_shuffle(on);
            }
            Command::Volume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.muted = false;
                self.apply_volume();
            }
            Command::Sleep(request) => {
                if self.sleep_fading {
                    self.sleep_fading = false;
                    self.fade = None;
                    self.fade_gain = 1.0;
                    self.apply_volume();
                }
                let (next_sleep, after_song) = match request {
                    None => (None, false),
                    Some(SleepRequest::AfterSong) => (Some(Sleep::AfterSong), true),
                    Some(SleepRequest::Seconds(secs)) => {
                        let bounded = if secs.is_finite() {
                            secs.clamp(1.0, 86_400.0)
                        } else {
                            1.0
                        };
                        (
                            Some(Sleep::At(Instant::now() + Duration::from_secs_f64(bounded))),
                            false,
                        )
                    }
                };
                self.sleep = next_sleep;
                self.queue.lock().expect("queue lock").sleep_after_song = after_song;
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
                if let Some(original) = queue.unshuffled.as_mut() {
                    let entry = (track.track_id.clone(), track.song_id.clone());
                    if next {
                        original.insert(0, entry);
                    } else {
                        original.push(entry);
                    }
                }
                queue.tracks.insert(at, *track);
                queue.coming = shifted(queue.coming, at);
            }
            Command::Settings {
                volume,
                muted,
                levelling,
                crossfade,
                visualizer,
            } => {
                self.volume = volume.clamp(0.0, 1.0);
                self.muted = muted;
                self.levelling = levelling;
                self.crossfade = crossfade.max(0.0);
                if self.visualizer != visualizer {
                    self.visualizer = visualizer;
                    if let Some(ref spec) = self.spectrum_element {
                        spec.set_property("post-messages", visualizer.on);
                    }
                    self.levels.clear();
                    if !visualizer.on {
                        self.heard_ahead.clear();
                    }
                    self.show_spectrum();
                }
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
            Command::Remove(ahead) => {
                let needs_reload = {
                    let mut queue = self.queue.lock().expect("queue lock");
                    let at = queue.at;
                    let coming = queue.coming;
                    let Queue {
                        tracks, unshuffled, ..
                    } = &mut *queue;
                    if let Some((next_coming, reload)) =
                        remove_upcoming(tracks, at, ahead, coming, unshuffled)
                    {
                        queue.coming = next_coming;
                        reload
                    } else {
                        false
                    }
                };
                if needs_reload {
                    self.reload_current_in_place();
                }
            }
            Command::Move(from, to) => {
                let needs_reload = {
                    let mut queue = self.queue.lock().expect("queue lock");
                    let at = queue.at;
                    let coming = queue.coming;
                    let repeat = queue.repeat.clone();
                    if let Some((next_coming, reload)) =
                        move_upcoming(&mut queue.tracks, at, from, to, coming, &repeat)
                    {
                        queue.coming = next_coming;
                        reload
                    } else {
                        false
                    }
                };
                if needs_reload {
                    self.reload_current_in_place();
                }
            }
            Command::Stop => {
                let _ = self.playbin.set_state(gst::State::Null);
                trim_heap();
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
                    if moved && matches!(self.sleep, Some(Sleep::AfterSong)) {
                        self.sleep = None;
                        self.queue.lock().expect("queue lock").sleep_after_song = false;
                        self.seek_target.set(None);
                        self.last_shown.set(0.0);
                        self.heard = 0.0;
                        self.last = None;
                        self.counted = false;
                        self.set_playing(false);
                        self.seek(0.0);
                        self.apply_volume();
                    } else if moved {
                        self.seek_target.set(None);
                        self.last_shown.set(0.0);
                        self.started();
                    }
                    self.failures = 0;
                }
                gst::MessageView::Eos(_) => {
                    if matches!(self.sleep, Some(Sleep::AfterSong)) {
                        self.sleep = None;
                        let next = {
                            let mut queue = self.queue.lock().expect("queue lock");
                            queue.sleep_after_song = false;
                            if queue.repeat != "one" {
                                next_index(queue.at, queue.tracks.len(), &queue.repeat)
                            } else {
                                None
                            }
                        };
                        if let Some(next) = next {
                            {
                                let mut queue = self.queue.lock().expect("queue lock");
                                queue.at = next;
                                queue.coming = None;
                            }
                            self.want_playing = false;
                            let _ = self.playbin.set_state(gst::State::Null);
                            self.released = true;
                            self.resume_time = 0.0;
                            self.last_shown.set(0.0);
                            self.seek_target.set(None);
                            self.heard = 0.0;
                            self.last = None;
                            self.counted = false;
                            self.paused_since = Some(Instant::now());
                        } else {
                            self.want_playing = false;
                            let _ = self.playbin.set_state(gst::State::Paused);
                            self.paused_since = Some(Instant::now());
                        }
                    } else {
                        self.want_playing = false;
                        let _ = self.playbin.set_state(gst::State::Paused);
                        self.paused_since = Some(Instant::now());
                    }
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
                gst::MessageView::Element(element) if self.visualizer.on => {
                    if let Some(heard) = element.structure().and_then(read_spectrum) {
                        self.heard_ahead.push_back(heard);
                    }
                }
                _ => {}
            }
        }
    }

    fn show_spectrum(&mut self) {
        let live = self.visualizer.on
            && self.spectrum_element.is_some()
            && self.want_playing
            && !self.released;
        if live {
            if let Some(mags) = self
                .playbin
                .current_running_time()
                .and_then(|now| due_spectrum(&mut self.heard_ahead, now))
            {
                self.levels = normalize_spectrum_db(
                    &mags,
                    Some(&self.levels),
                    self.visualizer.bars,
                    self.visualizer.bounce,
                );
            }
            self.levels.resize(self.visualizer.bars, SPECTRUM_REST);
        }
        self.shown.lock().expect("shown lock").spectrum = live.then(|| self.levels.clone());
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
            trim_heap();
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
        let playing = self.want_playing && !self.released;
        let effective_volume = if self.muted { 0.0 } else { self.volume };
        let preview = Preview {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            cover: cover_for(&self.server, track),
            playing,
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
            shuffle: Some(queue.unshuffled.is_some()),
            repeat: Some(queue.repeat.clone()),
            volume: Some(effective_volume),
            up_next: next.map(|track| UpNext {
                title: track.title.clone(),
                artist: track.artist.clone(),
            }),
        };
        let snapshot = Snapshot {
            track: track.clone(),
            playing,
            time: position,
            duration: self.stream_length().or(track.duration),
            volume: effective_volume,
            repeat: queue.repeat.clone(),
            shuffle: queue.unshuffled.is_some(),
            sleep: sleep_snapshot(self.sleep, Instant::now()),
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
) -> Result<Player, String> {
    gst::init().map_err(|error| error.to_string())?;
    let relay = Relay::start(server.clone(), token)?;

    let mut first = Queue {
        tracks: handover.tracks.clone(),
        at: handover.position,
        repeat: handover.repeat.clone(),
        coming: None,
        unshuffled: None,
        sleep_after_song: false,
    };
    if handover.restored {
        if handover.shuffle {
            first.unshuffled = handover.unshuffled.clone().or_else(|| {
                let from = (handover.position + 1).min(handover.tracks.len());
                Some(
                    handover.tracks[from..]
                        .iter()
                        .map(|track| (track.track_id.clone(), track.song_id.clone()))
                        .collect(),
                )
            });
        }
    } else if handover.shuffle {
        first.set_shuffle(true);
    }
    let queue = Arc::new(Mutex::new(first));
    let (playbin, active, spectrum_element) = make_playbin(&queue, &relay, handover.visualizer.on)?;

    let shown = Arc::new(Mutex::new(Shown {
        preview: None,
        snapshot: None,
        spectrum: None,
    }));
    let (commands, receive) = channel::<Command>();
    let restored_paused = handover.restored && !handover.playing;
    let initial_time = handover.time.max(0.0);
    let mut engine = Engine {
        playbin,
        spectrum_element,
        visualizer: handover.visualizer,
        heard_ahead: VecDeque::new(),
        levels: Vec::new(),
        active,
        outgoing: None,
        crossfade: handover.crossfade.max(0.0),
        queue: queue.clone(),
        shown: shown.clone(),
        server,
        relay,
        volume: handover.volume.clamp(0.0, 1.0),
        muted: handover.muted,
        levelling: handover.levelling,
        want_playing: handover.playing,
        seek_when_ready: None,
        seek_target: std::cell::Cell::new(None),
        last_shown: std::cell::Cell::new(initial_time),
        released: restored_paused,
        resume_time: if restored_paused { initial_time } else { 0.0 },
        paused_since: (!handover.playing).then(Instant::now),
        heard: 0.0,
        last: None,
        counted: false,
        failures: 0,
        report,
        fade_gain: 1.0,
        fade: None,
        sleep: None,
        sleep_fading: false,
    };
    std::thread::Builder::new()
        .name("soundscape-player".into())
        .spawn(move || {
            if restored_paused {
                engine.publish();
            } else {
                engine.load(handover.position, handover.time);
                engine.publish();
            }
            loop {
                let base_wait = if engine.fade.is_some() || engine.outgoing.is_some() {
                    Some(FADE_TICK)
                } else if engine.want_playing && engine.visualizer.on {
                    Some(SPECTRUM_TICK)
                } else if engine.want_playing
                    || engine.seek_when_ready.is_some()
                    || engine.seek_target.get().is_some()
                {
                    Some(TICK)
                } else if engine.released {
                    None
                } else {
                    let remaining = engine.paused_since.map_or(RELEASE_AFTER, |since| {
                        RELEASE_AFTER.saturating_sub(since.elapsed())
                    });
                    Some(remaining.clamp(Duration::from_millis(250), Duration::from_secs(1)))
                };
                let sleep_wait = match engine.sleep {
                    Some(Sleep::At(deadline)) => Some(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .clamp(Duration::from_millis(20), Duration::from_secs(1)),
                    ),
                    _ => None,
                };
                let wait = match (base_wait, sleep_wait) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                let received = match wait {
                    Some(timeout) => match receive.recv_timeout(timeout) {
                        Ok(command) => Some(command),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => {
                            engine.drop_outgoing();
                            let _ = engine.playbin.set_state(gst::State::Null);
                            trim_heap();
                            break;
                        }
                    },
                    None => match receive.recv() {
                        Ok(command) => Some(command),
                        Err(_) => {
                            engine.drop_outgoing();
                            let _ = engine.playbin.set_state(gst::State::Null);
                            trim_heap();
                            break;
                        }
                    },
                };
                let was_command = received.is_some();
                if let Some(command) = received {
                    if !engine.handle(command) {
                        break;
                    }
                }
                let prev_released = engine.released;
                let was_playing = engine.want_playing;
                let prev_sleep = engine.sleep;
                engine.step_sleep();
                if engine.want_playing
                    || engine.fade.is_some()
                    || engine.outgoing.is_some()
                    || engine.seek_when_ready.is_some()
                    || engine.sleep.is_some()
                    || !engine.released
                {
                    engine.step_fade();
                    engine.step_outgoing();
                    engine.crossfade_soon();
                    engine.messages();
                    engine.seek_when_ready();
                    engine.listen();
                    engine.release_when_quiet();
                }
                if was_command
                    || was_playing
                    || engine.want_playing
                    || engine.fade.is_some()
                    || engine.outgoing.is_some()
                    || engine.released != prev_released
                    || engine.sleep != prev_sleep
                {
                    engine.publish();
                }
                engine.show_spectrum();
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(Player {
        commands,
        shown,
        queue,
    })
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
    fn scales_volume_like_the_web_player() {
        assert_eq!(volume_gain(0.0), 0.0);
        assert_eq!(volume_gain(1.0), 1.0);
        assert!((volume_gain(0.05) - 1.25e-4).abs() < 1e-12);
        assert!((volume_gain(0.5) - 0.125).abs() < 1e-12);
        assert!((volume_gain(0.8) - 0.512).abs() < 1e-12);
        assert_eq!(volume_gain(-0.2), 0.0);
        assert_eq!(volume_gain(1.5), 1.0);
        assert_eq!(volume_gain(f64::NAN), 0.0);
        assert_eq!(volume_gain(f64::INFINITY), 0.0);
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
    fn maps_media_controls_to_the_player() {
        assert!(matches!(command_for("toggle", 0.0), Some(Command::Toggle)));
        assert!(matches!(command_for("seek", 5.0), Some(Command::SeekBy(s)) if s == 5.0));
        assert!(matches!(command_for("position", 30.0), Some(Command::SeekTo(s)) if s == 30.0));
        assert!(matches!(
            command_for("shuffle", 0.0),
            Some(Command::Shuffle(None))
        ));
    }

    fn named(ids: &[&str]) -> Vec<Track> {
        ids.iter()
            .map(|id| Track {
                track_id: (*id).into(),
                song_id: Some(format!("nd-{id}")),
                title: (*id).into(),
                artist: "Band".into(),
                album: None,
                cover: None,
                gain: None,
                duration: None,
            })
            .collect()
    }

    fn ids(tracks: &[Track]) -> Vec<&str> {
        tracks.iter().map(|track| track.track_id.as_str()).collect()
    }

    #[test]
    fn shuffles_only_what_is_still_to_come_and_puts_it_back() {
        let mut queue = Queue {
            tracks: named(&["a", "b", "c", "d", "e", "f", "g", "h"]),
            at: 1,
            repeat: "off".into(),
            coming: None,
            unshuffled: None,
            sleep_after_song: false,
        };
        queue.set_shuffle(true);
        assert_eq!(ids(&queue.tracks[..2]), ["a", "b"]);
        let mut rest: Vec<&str> = ids(&queue.tracks[2..]);
        assert_ne!(rest, ["c", "d", "e", "f", "g", "h"]);
        rest.sort();
        assert_eq!(rest, ["c", "d", "e", "f", "g", "h"]);
        queue.at = 3;
        let playing = queue.tracks[3].track_id.clone();
        queue.set_shuffle(false);
        assert!(queue.unshuffled.is_none());
        assert_eq!(queue.tracks[3].track_id, playing);
        let after = ids(&queue.tracks[4..]);
        let mut sorted = after.clone();
        sorted.sort();
        assert_eq!(after, sorted);
        assert_eq!(queue.tracks.len(), 8);
    }

    #[test]
    fn keeps_the_song_already_lined_up_next() {
        let mut queue = Queue {
            tracks: named(&["a", "b", "c", "d", "e"]),
            at: 0,
            repeat: "off".into(),
            coming: Some(1),
            unshuffled: None,
            sleep_after_song: false,
        };
        queue.set_shuffle(true);
        assert_eq!(ids(&queue.tracks[..2]), ["a", "b"]);
    }

    #[test]
    fn puts_songs_back_in_order_with_new_ones_last() {
        let original: Vec<(String, Option<String>)> = ["c", "d", "e"]
            .into_iter()
            .map(|id| (id.to_string(), Some(format!("nd-{id}"))))
            .collect();
        let upcoming = named(&["x", "e", "c"]);
        assert_eq!(ids(&restore_order(&original, upcoming)), ["c", "e", "x"]);
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
    fn removes_and_reorders_upcoming_songs() {
        let mut tracks = named(&["a", "b", "c", "d"]);
        let mut unshuffled = Some(vec![
            ("b".into(), Some("nd-b".into())),
            ("c".into(), Some("nd-c".into())),
            ("d".into(), Some("nd-d".into())),
        ]);
        assert_eq!(
            remove_upcoming(&mut tracks, 0, 0, None, &mut unshuffled),
            None
        );
        assert_eq!(
            remove_upcoming(&mut tracks, 0, 4, None, &mut unshuffled),
            None
        );
        assert_eq!(
            remove_upcoming(&mut tracks, 0, 2, Some(3), &mut unshuffled),
            Some((Some(2), false))
        );
        assert_eq!(ids(&tracks), ["a", "b", "d"]);
        assert_eq!(
            unshuffled,
            Some(vec![
                ("b".into(), Some("nd-b".into())),
                ("d".into(), Some("nd-d".into())),
            ])
        );
        assert_eq!(
            remove_upcoming(&mut tracks, 0, 1, Some(1), &mut unshuffled),
            Some((None, true))
        );
        assert_eq!(ids(&tracks), ["a", "d"]);

        let mut list = named(&["a", "b", "c", "d"]);
        assert_eq!(move_upcoming(&mut list, 0, 0, 1, None, "off"), None);
        assert_eq!(move_upcoming(&mut list, 0, 2, 2, None, "off"), None);
        assert_eq!(move_upcoming(&mut list, 0, 1, 4, None, "off"), None);
        assert_eq!(
            move_upcoming(&mut list, 0, 3, 2, Some(1), "off"),
            Some((Some(1), false))
        );
        assert_eq!(ids(&list), ["a", "b", "d", "c"]);
        assert_eq!(
            move_upcoming(&mut list, 0, 2, 1, Some(1), "off"),
            Some((None, true))
        );
        assert_eq!(ids(&list), ["a", "d", "b", "c"]);
        assert_eq!(
            move_upcoming(&mut list, 0, 1, 3, Some(1), "off"),
            Some((None, true))
        );
        assert_eq!(ids(&list), ["a", "b", "c", "d"]);
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

    #[test]
    fn shows_each_reading_when_it_is_heard() {
        let ms = gst::ClockTime::from_mseconds;
        let mut ahead = VecDeque::from([
            (ms(90_000), vec![9.0]),
            (ms(100), vec![1.0]),
            (ms(145), vec![2.0]),
            (ms(300), vec![3.0]),
        ]);
        assert_eq!(due_spectrum(&mut ahead, ms(50)), None);
        assert_eq!(ahead.len(), 3, "a reading from before a seek has to go");
        assert_eq!(due_spectrum(&mut ahead, ms(150)), Some(vec![2.0]));
        assert_eq!(due_spectrum(&mut ahead, ms(200)), None);
        assert_eq!(due_spectrum(&mut ahead, ms(300)), Some(vec![3.0]));
    }

    #[test]
    fn reads_and_normalizes_spectrum_messages_from_gstreamer() {
        let _ = gst::init();
        let silent = vec![-70.0f32; 128];
        let shape = |mags: &[f32], previous: Option<&[f32]>| {
            normalize_spectrum_db(mags, previous, 8, Bounce::Lively)
        };
        assert_eq!(shape(&silent, None), [SPECTRUM_REST; 8]);
        for bars in SPECTRUM_BAR_CHOICES {
            assert_eq!(
                normalize_spectrum_db(&silent, None, bars, Bounce::Normal).len(),
                bars
            );
        }

        let mut bass_hit = vec![-70.0f32; 128];
        bass_hit[0] = -6.0;
        let bars = shape(&bass_hit, None);
        assert!(bars[0] > 0.85, "bass bar was {}", bars[0]);
        assert_eq!(bars[7], 0.15);
        let mut loud = silent.clone();
        loud[0] = -15.0;
        for bounce in [
            Bounce::Gentle,
            Bounce::Normal,
            Bounce::Lively,
            Bounce::Extreme,
        ] {
            let level = normalize_spectrum_db(&loud, None, 8, bounce)[0];
            assert!(level < 0.97, "loud bass filled the {bounce:?} bar: {level}");
        }
        let decayed = shape(&silent, Some(&bars));
        assert!(decayed[0] < bars[0] && decayed[0] > 0.15);

        let Ok(pipeline) = gst::parse::launch(
            "audiotestsrc num-buffers=5 wave=sine freq=440 ! spectrum bands=128 threshold=-70 interval=10000000 post-messages=true ! fakesink",
        ) else {
            return;
        };
        let _ = pipeline.set_state(gst::State::Playing);
        let mut received = None;
        if let Some(bus) = pipeline.bus() {
            for _ in 0..30 {
                let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(50)) else {
                    break;
                };
                if let gst::MessageView::Element(element) = msg.view() {
                    if let Some((_, mags)) = element.structure().and_then(read_spectrum) {
                        received = Some(mags);
                        break;
                    }
                }
            }
        }
        let _ = pipeline.set_state(gst::State::Null);
        let mags = received.expect("GStreamer spectrum element should emit magnitude list");
        assert_eq!(mags.len(), 128);
        let normalized = shape(&mags, None);
        assert!(
            normalized.iter().any(|&v| v > 0.4),
            "440Hz sine wave should raise a spectrum bar: {normalized:?}"
        );
    }

    #[test]
    fn describes_sleep_snapshots_and_saved_queues() {
        let now = Instant::now();
        assert_eq!(sleep_snapshot(None, now), None);
        assert_eq!(
            sleep_snapshot(Some(Sleep::AfterSong), now),
            Some(SleepSnapshot::AfterSong)
        );
        let later = sleep_snapshot(Some(Sleep::At(now + Duration::from_secs(90))), now);
        assert!(
            matches!(later, Some(SleepSnapshot::Remaining(secs)) if (secs - 90.0).abs() < 0.01)
        );
        assert!(matches!(
            sleep_snapshot(Some(Sleep::At(now - Duration::from_secs(5))), now),
            Some(SleepSnapshot::Remaining(secs)) if secs == 0.0
        ));

        let saved = SavedQueue {
            server: "https://music.example.com/".into(),
            tracks: vec![track("One", Some("Album"))],
            position: 0,
            time: 42.5,
            repeat: "all".into(),
            shuffle: true,
            unshuffled: Some(vec![("band-one".into(), Some("nd-One".into()))]),
        };
        let encoded = serde_json::to_string(&saved).unwrap();
        let decoded: SavedQueue = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, saved);
    }
}
