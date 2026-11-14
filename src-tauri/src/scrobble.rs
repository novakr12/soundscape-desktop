use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const MIN_TRACK_MS: u64 = 30_000;
pub const MAX_WAIT_MS: u64 = 240_000;
const MAX_GAP_MS: i64 = 10_000;
const RESTART_MS: u64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub artist: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<u64>,
}

impl Track {
    pub fn same_song(&self, other: &Track) -> bool {
        self.artist == other.artist && self.title == other.title && self.album == other.album
    }

    pub fn needed_ms(&self) -> Option<u64> {
        match self.length_ms {
            Some(length) if length < MIN_TRACK_MS => None,
            Some(length) => Some((length / 2).min(MAX_WAIT_MS)),
            None => Some(MAX_WAIT_MS),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub player: String,
    pub playing: bool,
    pub track: Option<Track>,
    pub position_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    NowPlaying {
        player: String,
        track: Track,
    },
    Scrobble {
        player: String,
        track: Track,
        listened_at: i64,
    },
}

#[derive(Default)]
struct PlayerState {
    track: Option<Track>,
    started_at_ms: i64,
    heard_ms: u64,
    last_seen_ms: i64,
    last_position_ms: Option<u64>,
    playing: bool,
    announced: bool,
    scrobbled: bool,
}

impl PlayerState {
    fn start(&mut self, track: Option<Track>, now_ms: i64, position_ms: Option<u64>) {
        self.track = track;
        self.started_at_ms = now_ms - position_ms.unwrap_or(0).min(MAX_WAIT_MS * 4) as i64;
        self.heard_ms = 0;
        self.announced = false;
        self.scrobbled = false;
    }
}

#[derive(Default)]
pub struct Engine {
    players: HashMap<String, PlayerState>,
}

impl Engine {
    pub fn skip(&mut self, track: &Track) -> bool {
        let mut skipped = false;
        for state in self.players.values_mut() {
            if !state.scrobbled && state.track.as_ref().is_some_and(|t| t.same_song(track)) {
                state.scrobbled = true;
                skipped = true;
            }
        }
        skipped
    }

    pub fn counted(&self, track: &Track) -> bool {
        self.players.values().any(|state| {
            state.scrobbled && state.track.as_ref().is_some_and(|t| t.same_song(track))
        })
    }

    pub fn update(&mut self, now_ms: i64, snapshots: &[Snapshot]) -> Vec<Event> {
        self.players
            .retain(|player, _| snapshots.iter().any(|snapshot| &snapshot.player == player));
        let mut events = Vec::new();
        for snapshot in snapshots {
            let fresh = !self.players.contains_key(&snapshot.player);
            let state = self.players.entry(snapshot.player.clone()).or_default();
            if !fresh && state.playing && state.track.is_some() {
                let gap = (now_ms - state.last_seen_ms).clamp(0, MAX_GAP_MS) as u64;
                state.heard_ms += gap;
            }

            let changed = match (&state.track, &snapshot.track) {
                (Some(old), Some(new)) => !old.same_song(new),
                (None, None) => false,
                _ => true,
            };
            let restarted = !changed
                && matches!((state.last_position_ms, snapshot.position_ms), (Some(before), Some(now)) if now + RESTART_MS / 2 < before && now < RESTART_MS)
                && (state.scrobbled || state.heard_ms > RESTART_MS);
            if fresh || changed || restarted {
                state.start(snapshot.track.clone(), now_ms, snapshot.position_ms);
            } else if let (Some(old), Some(new)) = (&mut state.track, &snapshot.track) {
                if old.length_ms.is_none() {
                    old.length_ms = new.length_ms;
                }
            }

            if snapshot.playing && !state.playing {
                state.announced = false;
            }
            state.playing = snapshot.playing;
            state.last_seen_ms = now_ms;
            state.last_position_ms = snapshot.position_ms;

            let Some(track) = state.track.clone() else {
                continue;
            };
            if state.playing && !state.announced {
                state.announced = true;
                events.push(Event::NowPlaying {
                    player: snapshot.player.clone(),
                    track: track.clone(),
                });
            }
            if let Some(needed) = track.needed_ms() {
                if !state.scrobbled && state.heard_ms >= needed {
                    state.scrobbled = true;
                    events.push(Event::Scrobble {
                        player: snapshot.player.clone(),
                        track,
                        listened_at: state.started_at_ms.div_euclid(1000),
                    });
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(title: &str, length_s: Option<u64>) -> Track {
        Track {
            artist: "Radiohead".into(),
            title: title.into(),
            album: Some("OK Computer".into()),
            length_ms: length_s.map(|s| s * 1000),
        }
    }

    fn snap(track: Option<Track>, playing: bool, position_s: Option<u64>) -> Vec<Snapshot> {
        vec![Snapshot {
            player: "strawberry".into(),
            playing,
            track,
            position_ms: position_s.map(|s| s * 1000),
        }]
    }

    fn scrobbles(events: &[Event]) -> Vec<(String, i64)> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Scrobble {
                    track, listened_at, ..
                } => Some((track.title.clone(), *listened_at)),
                _ => None,
            })
            .collect()
    }

    fn run(
        engine: &mut Engine,
        from_s: i64,
        to_s: i64,
        step_s: i64,
        f: impl Fn(i64) -> Vec<Snapshot>,
    ) -> Vec<Event> {
        let mut events = Vec::new();
        let mut t = from_s;
        while t <= to_s {
            events.extend(engine.update(t * 1000, &f(t)));
            t += step_s;
        }
        events
    }

    #[test]
    fn a_skipped_song_is_never_scrobbled_but_counts_again_when_replayed() {
        let mut engine = Engine::default();
        let track = song("Skip me", Some(200));
        engine.update(0, &snap(Some(track.clone()), true, Some(0)));
        assert!(!engine.counted(&track));
        assert!(engine.skip(&track));
        assert!(engine.counted(&track));
        assert!(!engine.skip(&track));
        let mut events = Vec::new();
        for second in 1..=150 {
            events.extend(engine.update(
                second * 1000,
                &snap(Some(track.clone()), true, Some(second as u64)),
            ));
        }
        assert!(scrobbles(&events).is_empty());
        engine.update(151_000, &snap(Some(track.clone()), true, Some(1)));
        assert!(!engine.counted(&track));
        assert!(!engine.skip(&song("Something else", Some(200))));
    }

    #[test]
    fn needs_half_the_song_or_four_minutes() {
        assert_eq!(song("a", Some(200)).needed_ms(), Some(100_000));
        assert_eq!(song("a", Some(600)).needed_ms(), Some(240_000));
        assert_eq!(song("a", None).needed_ms(), Some(240_000));
        assert_eq!(song("a", Some(29)).needed_ms(), None);
    }

    #[test]
    fn announces_then_scrobbles_once_at_the_start_time() {
        let mut engine = Engine::default();
        let track = song("Karma Police", Some(264));
        let start = 1_700_000_000;
        let events = run(&mut engine, start, start + 300, 2, |t| {
            snap(Some(track.clone()), true, Some((t - start) as u64))
        });
        assert!(
            matches!(&events[0], Event::NowPlaying { track, .. } if track.title == "Karma Police")
        );
        assert_eq!(
            scrobbles(&events),
            vec![("Karma Police".to_string(), start)]
        );
    }

    #[test]
    fn counts_the_time_from_when_the_song_really_started() {
        let mut engine = Engine::default();
        let track = song("Lucky", Some(260));
        let opened = 1_700_000_000;
        let events = run(&mut engine, opened, opened + 200, 2, |t| {
            snap(Some(track.clone()), true, Some((t - opened + 40) as u64))
        });
        assert_eq!(scrobbles(&events), vec![("Lucky".to_string(), opened - 40)]);
    }

    #[test]
    fn paused_time_doesnt_count() {
        let mut engine = Engine::default();
        let track = song("Airbag", Some(284));
        let events = run(&mut engine, 0, 400, 2, |t| {
            snap(
                Some(track.clone()),
                !(60..=360).contains(&t),
                Some(t.min(60) as u64),
            )
        });
        assert_eq!(scrobbles(&events), vec![]);
        let more = run(&mut engine, 402, 600, 2, |t| {
            snap(Some(track.clone()), true, Some((t - 342) as u64))
        });
        assert_eq!(scrobbles(&more).len(), 1);
    }

    #[test]
    fn skipping_early_scrobbles_nothing_and_the_next_song_starts_fresh() {
        let mut engine = Engine::default();
        let first = song("First", Some(200));
        let second = song("Second", Some(200));
        let events = run(&mut engine, 0, 300, 2, |t| {
            if t < 50 {
                snap(Some(first.clone()), true, Some(t as u64))
            } else {
                snap(Some(second.clone()), true, Some((t - 50) as u64))
            }
        });
        assert_eq!(scrobbles(&events), vec![("Second".to_string(), 50)]);
        let announced = events
            .iter()
            .filter(|e| matches!(e, Event::NowPlaying { .. }))
            .count();
        assert_eq!(announced, 2);
    }

    #[test]
    fn a_repeated_song_counts_again() {
        let mut engine = Engine::default();
        let track = song("Loop", Some(120));
        let events = run(&mut engine, 0, 260, 2, |t| {
            snap(Some(track.clone()), true, Some((t % 120) as u64))
        });
        assert_eq!(
            scrobbles(&events),
            vec![("Loop".to_string(), 0), ("Loop".to_string(), 120)]
        );
    }

    #[test]
    fn a_long_sleep_doesnt_count_as_listening() {
        let mut engine = Engine::default();
        let track = song("Long", Some(600));
        let mut events = engine.update(0, &snap(Some(track.clone()), true, Some(0)));
        events.extend(engine.update(3_600_000, &snap(Some(track.clone()), true, Some(10))));
        assert_eq!(scrobbles(&events), vec![]);
    }

    #[test]
    fn short_clips_never_count() {
        let mut engine = Engine::default();
        let clip = song("Jingle", Some(20));
        let events = run(&mut engine, 0, 100, 2, |t| {
            snap(Some(clip.clone()), true, Some((t % 20) as u64))
        });
        assert_eq!(scrobbles(&events), vec![]);
    }

    #[test]
    fn players_are_tracked_separately_and_forgotten_when_they_close() {
        let mut engine = Engine::default();
        let a = song("From A", Some(100));
        let b = song("From B", Some(100));
        let events = run(&mut engine, 0, 60, 2, |t| {
            vec![
                Snapshot {
                    player: "a".into(),
                    playing: true,
                    track: Some(a.clone()),
                    position_ms: Some(t as u64 * 1000),
                },
                Snapshot {
                    player: "b".into(),
                    playing: t < 20,
                    track: Some(b.clone()),
                    position_ms: Some(t.min(20) as u64 * 1000),
                },
            ]
        });
        assert_eq!(scrobbles(&events), vec![("From A".to_string(), 0)]);
        engine.update(62_000, &[]);
        assert!(engine.players.is_empty());
    }
}
