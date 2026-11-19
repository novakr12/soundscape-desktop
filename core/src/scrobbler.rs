use crate::host::{AppHandle, Manager};
use crate::mpris::{self, Player};
use crate::queue::Queue;
use crate::scrobble::{Engine, Event, Track};
use crate::submit::{self, Failure, Listen, BATCH};
use crate::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

const EVERY: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerChoice {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub scrobbling: bool,
    pub connected: bool,
    pub listening: Option<(Track, String)>,
    pub original: Option<Track>,
    pub counted: bool,
    pub paused_minutes: Option<u64>,
    pub players: Vec<PlayerChoice>,
    pub waiting: usize,
    pub signed_out: bool,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn enabled(players: &std::collections::BTreeMap<String, bool>, id: &str) -> bool {
    players
        .get(id)
        .copied()
        .unwrap_or_else(|| mpris::on_by_default(id))
}

pub fn listening(found: &[Player], allowed: impl Fn(&str) -> bool) -> Option<(Track, String)> {
    found
        .iter()
        .filter(|player| allowed(&player.id) && player.snapshot.playing)
        .find_map(|player| {
            player
                .snapshot
                .track
                .clone()
                .map(|track| (track, player.name.clone()))
        })
}

static SENDING: AtomicBool = AtomicBool::new(false);

pub async fn send_waiting(app: &AppHandle) -> Result<usize, Failure> {
    let state = app.state::<AppState>();
    let queue = Queue::new(&state.dir);
    let config = state.config();
    let (Some(server), Some(token)) = (
        config.server.as_deref().and_then(|s| Url::parse(s).ok()),
        config.token,
    ) else {
        return Ok(queue.len());
    };
    if SENDING.swap(true, Ordering::AcqRel) {
        return Ok(queue.len());
    }
    let result = loop {
        let batch = queue.first(BATCH);
        if batch.is_empty() {
            break Ok(0);
        }
        match submit::listens(&server, &token, &batch).await {
            Ok(()) | Err(Failure::Refused(_)) => {
                let _ = queue.drop_first(batch.len());
            }
            Err(Failure::SignedOut) => {
                state.set_signed_out(true);
                break Err(Failure::SignedOut);
            }
            Err(Failure::Offline) => break Err(Failure::Offline),
        }
    };
    SENDING.store(false, Ordering::Release);
    result
}

pub fn focus(
    found: &[Player],
    allowed: impl Fn(&str) -> bool,
    last: Option<&str>,
) -> Option<Player> {
    found
        .iter()
        .find(|player| {
            allowed(&player.id) && player.snapshot.playing && player.snapshot.track.is_some()
        })
        .or_else(|| {
            found
                .iter()
                .find(|player| Some(player.id.as_str()) == last && player.snapshot.track.is_some())
        })
        .cloned()
}

pub async fn run(app: AppHandle) {
    let mut engine = Engine::default();
    let mut last_focus: Option<String> = None;
    let mut connection: Option<zbus::Connection> = None;
    let mut retry_after: Option<Instant> = None;
    loop {
        tokio::time::sleep(EVERY).await;
        let state = app.state::<AppState>();
        let config = state.config();
        let queue = Queue::new(&state.dir);

        if connection.is_none() {
            connection = zbus::Connection::session().await.ok();
        }
        let found = match &connection {
            Some(bus) => match mpris::players(bus).await {
                Ok(found) => found,
                Err(_) => {
                    connection = None;
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let allowed = |id: &str| enabled(&config.players, id);
        let now = crate::config::now_seconds();
        if config.paused_until.is_some() && config.paused_minutes(now).is_none() {
            state.update(|config| config.paused_until = None);
        }
        let original = if config.sending(now) {
            listening(&found, allowed).map(|(track, _)| track)
        } else {
            None
        };
        let found: Vec<Player> = found
            .into_iter()
            .map(|mut player| {
                player.snapshot.track = player
                    .snapshot
                    .track
                    .map(|track| crate::fixes::apply(&config.fixes, track));
                player
            })
            .collect();
        let active: Vec<_> = found
            .iter()
            .filter(|player| allowed(&player.id))
            .map(|player| player.snapshot.clone())
            .collect();
        let running = config.sending(now);
        let focused = focus(&found, allowed, last_focus.as_deref());
        last_focus = focused.as_ref().map(|player| player.id.clone());
        state.set_mini(focused.and_then(|player| {
            Some(crate::mini::Source {
                track: player.snapshot.track?,
                bus: player.bus,
                name: player.name,
                art: player.art,
                playing: player.snapshot.playing,
                position_ms: player.snapshot.position_ms,
            })
        }));
        if let Some(track) = state.take_skip_request() {
            if engine.skip(&track) {
                state.set_skipped(Some(track));
            }
        }
        let events = engine.update(now_ms(), if running { &active } else { &[] });
        let server = config
            .server
            .as_deref()
            .and_then(|server| Url::parse(server).ok());
        let token = config.token.clone();

        let mut scrobbled = false;
        for event in events {
            match event {
                Event::NowPlaying { player, track } => {
                    if let (Some(server), Some(token)) = (server.clone(), token.clone()) {
                        let name = found
                            .iter()
                            .find(|p| p.id == player)
                            .map(|p| p.name.clone())
                            .unwrap_or(player);
                        crate::host::async_runtime::spawn(async move {
                            let _ = submit::now_playing(&server, &token, &track, &name).await;
                        });
                    }
                }
                Event::Scrobble {
                    player,
                    track,
                    listened_at,
                } => {
                    let name = found
                        .iter()
                        .find(|p| p.id == player)
                        .map(|p| p.name.clone())
                        .unwrap_or(player);
                    if let Err(error) = queue.push(Listen {
                        listened_at,
                        track,
                        player: name,
                    }) {
                        eprintln!("Couldn't keep a play to send later: {error}");
                    }
                    scrobbled = true;
                }
            }
        }

        let due = scrobbled || retry_after.is_none_or(|at| Instant::now() >= at);
        if due && !state.signed_out() && !queue.is_empty() {
            match send_waiting(&app).await {
                Err(Failure::Offline) => retry_after = Some(Instant::now() + RETRY),
                Ok(sent) if sent > 0 => {
                    retry_after = None;
                    let app = app.clone();
                    crate::host::async_runtime::spawn(
                        async move { crate::today::check(&app).await },
                    );
                }
                _ => retry_after = None,
            }
        }
        let signed_out = state.signed_out();
        let now_listening = if running {
            listening(&found, allowed)
        } else {
            None
        };

        let status = Status {
            scrobbling: config.scrobbling,
            connected: config.connected(),
            counted: now_listening
                .as_ref()
                .is_some_and(|(track, _)| engine.counted(track)),
            listening: now_listening,
            paused_minutes: config.paused_minutes(now),
            original,
            players: found
                .iter()
                .map(|p| PlayerChoice {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    enabled: allowed(&p.id),
                })
                .collect(),
            waiting: queue.len(),
            signed_out,
        };
        if state.set_status(status) {
            crate::tray::refresh(&app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobble::Snapshot;
    use std::collections::BTreeMap;

    fn player(id: &str, playing: bool) -> Player {
        Player {
            id: id.into(),
            name: id.to_uppercase(),
            bus: format!("org.mpris.MediaPlayer2.{id}"),
            art: None,
            snapshot: Snapshot {
                player: id.into(),
                playing,
                track: Some(Track {
                    artist: "A".into(),
                    title: format!("from {id}"),
                    album: None,
                    length_ms: None,
                }),
                position_ms: None,
            },
        }
    }

    #[test]
    fn the_mini_player_follows_the_playing_player_and_stays_on_a_paused_one() {
        let everyone = |_: &str| true;
        let both = vec![player("spotify", false), player("strawberry", true)];
        assert_eq!(focus(&both, everyone, None).unwrap().id, "strawberry");
        let paused = vec![player("spotify", false), player("strawberry", false)];
        assert_eq!(
            focus(&paused, everyone, Some("strawberry")).unwrap().id,
            "strawberry"
        );
        assert_eq!(focus(&paused, everyone, None), None);
        assert_eq!(focus(&paused, everyone, Some("gone")), None);
        let only_browsers = |id: &str| id != "firefox";
        assert_eq!(focus(&[player("firefox", true)], only_browsers, None), None);
    }

    #[test]
    fn your_choices_win_over_the_defaults() {
        let mut choices = BTreeMap::new();
        assert!(enabled(&choices, "spotify"));
        assert!(!enabled(&choices, "firefox"));
        choices.insert("firefox".to_string(), true);
        choices.insert("spotify".to_string(), false);
        assert!(enabled(&choices, "firefox"));
        assert!(!enabled(&choices, "spotify"));
    }

    #[test]
    fn shows_the_first_allowed_player_that_is_playing() {
        let found = vec![
            player("firefox", true),
            player("rhythmbox", false),
            player("spotify", true),
        ];
        let choices = BTreeMap::new();
        let (track, name) = listening(&found, |id| enabled(&choices, id)).unwrap();
        assert_eq!(
            (track.title.as_str(), name.as_str()),
            ("from spotify", "SPOTIFY")
        );
        assert_eq!(listening(&found[..2], |id| enabled(&choices, id)), None);
    }
}
