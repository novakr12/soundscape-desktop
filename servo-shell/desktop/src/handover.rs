use crate::host::{AppHandle, Manager};
use crate::media::Preview;
use crate::native::{self, Command, Player};
use crate::{AppState, MAIN};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use url::Url;

static HANDBACKS: AtomicU64 = AtomicU64::new(0);

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
    with_player(app, |player| player.send(command)).is_some()
}

pub fn wanted(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let config = state.config();
    config.native_player && config.connected() && !state.signed_out()
}

fn report(server: Url, token: String) -> native::Report {
    Box::new(move |kind, track| {
        let server = server.clone();
        let token = token.clone();
        crate::host::async_runtime::spawn(async move {
            let path = if kind == "play" {
                "/api/play"
            } else {
                "/api/now-playing"
            };
            let Ok(address) = crate::server::address(&server, path) else {
                return;
            };
            let mut body = serde_json::json!({ "trackId": track.track_id });
            if let Some(song) = &track.song_id {
                body["songId"] = serde_json::Value::from(song.clone());
            }
            let _ =
                crate::server::answer(crate::server::client().post(address).json(&body), &token)
                    .await;
        });
    })
}

const START_LEAD: f64 = 1.0;
const FIRST_LEAD: f64 = 0.25;
const FADE: Duration = Duration::from_millis(300);
const FADE_STEPS: usize = 10;
const ROUNDS: usize = 6;

async fn page_clock(app: &AppHandle) -> Option<native::PageClock> {
    native::parse_page_clock(&crate::media::evaluate(app, native::PAGE_CLOCK).await?)
}

fn age(page: &native::PageClock) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    native::page_age(page.at, now)
}

async fn wait_moving(player: &Player, limit: Duration) -> bool {
    let began = std::time::Instant::now();
    let mut seen = 0;
    while began.elapsed() < limit {
        if player.now().is_some_and(|(_, _, moving)| moving) {
            seen += 1;
            if seen >= 2 {
                return true;
            }
        } else {
            seen = 0;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    false
}

async fn fade_in_page(app: &AppHandle) {
    let step = FADE / FADE_STEPS as u32;
    for gain in native::fade_steps(0.0, 1.0, FADE_STEPS) {
        let _ = crate::media::evaluate(app, native::page_hold(gain)).await;
        tokio::time::sleep(step).await;
    }
}

async fn fade_page(app: &AppHandle, from: f64, to: f64) {
    let step = FADE / FADE_STEPS as u32;
    for volume in native::fade_steps(from, to, FADE_STEPS) {
        let _ = crate::media::evaluate(app, native::page_volume(volume)).await;
        tokio::time::sleep(step).await;
    }
}

async fn line_up_with_page(
    app: &AppHandle,
    player: &Player,
    at: usize,
) -> Option<native::PageClock> {
    let mut lead = FIRST_LEAD;
    let mut moved = false;
    for _ in 0..ROUNDS {
        let page = page_clock(app).await?;
        let (native_at, native_time, _) = player.now()?;
        if page.position != at as i64 || native_at != at || !page.playing {
            return None;
        }
        let drift = native_time - age(&page) - page.time;
        if native::in_step(drift) {
            return Some(page);
        }
        if moved {
            lead = native::next_lead(lead, drift);
        }
        player.send(Command::SeekTo(page.time + lead));
        moved = true;
        tokio::time::sleep(Duration::from_millis(80)).await;
        if !wait_moving(player, Duration::from_secs(3)).await {
            return None;
        }
    }
    page_clock(app).await.filter(|page| page.playing)
}

pub async fn hand_over(app: AppHandle) {
    let Some((server, token)) = crate::server::credentials(&app) else {
        return;
    };
    let Some(handover) = crate::media::evaluate(&app, native::READ_HANDOVER)
        .await
        .and_then(|json| native::parse_handover(&json))
    else {
        return;
    };
    if !handover.playing {
        return;
    }
    let at = handover.position;
    let mut ahead = handover.clone();
    ahead.time += START_LEAD;
    let player = match native::start(
        server.clone(),
        token.clone(),
        ahead,
        report(server, token),
        true,
    ) {
        Ok(player) => player,
        Err(error) => {
            eprintln!("The built-in player couldn't start: {error}");
            return;
        }
    };
    let ready = wait_moving(&player, Duration::from_secs(8)).await;
    let lined_up = if ready {
        line_up_with_page(&app, &player, at).await
    } else {
        None
    };
    match lined_up {
        Some(page) => {
            player.send(Command::FadeTo(1.0, FADE));
            fade_page(&app, page.volume, 0.0).await;
            let _ = crate::media::evaluate(&app, native::page_pause_with_volume(page.volume)).await;
        }
        None => {
            let page = page_clock(&app).await;
            let _ = crate::media::evaluate(&app, native::PAUSE_PAGE).await;
            if let Some(page) = page.filter(|page| page.position == at as i64) {
                player.send(Command::SeekTo(page.time));
            }
            player.send(Command::FadeTo(1.0, Duration::ZERO));
        }
    }
    *app.state::<AppState>().native.lock().expect("native lock") = Some(player);
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    if window.is_visible().unwrap_or(true) {
        crate::host::async_runtime::spawn(take_back(app.clone()));
        return;
    }
    let _ = window.destroy();
}

pub fn handback_script(app: &AppHandle) -> Option<String> {
    let resume = with_player(app, Player::resume)?;
    let paused = native::Resume {
        playing: false,
        ..resume
    };
    Some(native::handback_script(
        &paused,
        HANDBACKS.fetch_add(1, Ordering::Relaxed) + crate::config::now_seconds() as u64,
    ))
}

fn drop_player(app: &AppHandle) -> Option<Player> {
    app.state::<AppState>()
        .native
        .lock()
        .expect("native lock")
        .take()
}

pub fn hand_back(app: &AppHandle) {
    drop(drop_player(app));
}

async fn page_ready(app: &AppHandle, at: usize, limit: Duration) -> Option<native::PageClock> {
    let began = std::time::Instant::now();
    while began.elapsed() < limit {
        if let Some(page) = page_clock(app).await {
            if page.ready && page.position == at as i64 {
                return Some(page);
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    None
}

async fn page_moving(app: &AppHandle, limit: Duration) -> Option<native::PageClock> {
    let began = std::time::Instant::now();
    let mut before: Option<f64> = None;
    while began.elapsed() < limit {
        if let Some(page) = page_clock(app).await.filter(|page| page.playing) {
            if before.is_some_and(|time| page.time > time) {
                return Some(page);
            }
            before = Some(page.time);
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    None
}

pub async fn take_back(app: AppHandle) {
    let Some((at, _, _)) = with_player(&app, Player::now).flatten() else {
        return;
    };
    let playing = with_player(&app, |player| player.resume().playing).unwrap_or(false);
    if !playing {
        hand_back(&app);
        return;
    }
    if page_ready(&app, at, Duration::from_secs(25))
        .await
        .is_none()
    {
        if let Some(player) = drop_player(&app) {
            if let Some((_, time, _)) = player.now() {
                drop(player);
                let _ = crate::media::evaluate(&app, native::page_play_at(time)).await;
                let _ = crate::media::evaluate(&app, native::PAGE_RELEASE).await;
            }
        }
        return;
    }
    let mut lead = FIRST_LEAD;
    let Some((_, time, _)) = with_player(&app, Player::now).flatten() else {
        return;
    };
    let _ = crate::media::evaluate(&app, native::page_play_at(time + lead)).await;
    let mut moving = false;
    for _ in 0..ROUNDS {
        let Some(page) = page_moving(&app, Duration::from_secs(4)).await else {
            break;
        };
        moving = true;
        let Some((native_at, native_time, _)) = with_player(&app, Player::now).flatten() else {
            break;
        };
        if native_at != at || page.position != at as i64 {
            break;
        }
        let drift = page.time - (native_time - age(&page));
        if native::in_step(drift) {
            break;
        }
        lead = native::next_lead(lead, drift);
        let _ = crate::media::evaluate(&app, native::page_move_to(native_time + lead)).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    if moving {
        with_player(&app, |player| player.send(Command::FadeTo(0.0, FADE)));
        fade_in_page(&app).await;
    }
    let _ = crate::media::evaluate(&app, native::PAGE_RELEASE).await;
    hand_back(&app);
}
