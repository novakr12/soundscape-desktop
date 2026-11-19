use crate::host::{AppHandle, Manager};
use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use url::Url;

const EVERY: Duration = Duration::from_secs(60);
const FIRST: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Today {
    pub plays: u64,
    pub listening_ms: u64,
    pub streak: u64,
    pub played_today: bool,
}

pub const REMINDER_HOUR: i32 = 21;

pub fn remind(today: &Today, hour: i32, date: &str, enabled: bool, last: Option<&str>) -> bool {
    enabled
        && today.streak > 0
        && !today.played_today
        && hour >= REMINDER_HOUR
        && last != Some(date)
}

pub fn reminder(today: &Today) -> (String, String) {
    (
        format!("Your {}-day streak ends at midnight", today.streak),
        "Play something today to keep it going.".into(),
    )
}

fn local_now() -> Option<(i32, String)> {
    let now = gstreamer::glib::DateTime::now_local().ok()?;
    let (year, month, day) = now.ymd();
    Some((now.hour(), format!("{year:04}-{month:02}-{day:02}")))
}

fn listening_time(ms: u64) -> String {
    let minutes = (ms + 30_000) / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

pub fn text(today: &Today) -> String {
    let mut parts = vec![match today.plays {
        0 => "Today: no plays yet".to_string(),
        1 => "Today: 1 play".to_string(),
        plays => format!("Today: {plays} plays"),
    }];
    if today.plays > 0 {
        parts.push(listening_time(today.listening_ms));
    }
    match (today.streak, today.played_today) {
        (0, _) => {}
        (days, true) => parts.push(format!("🔥 {days}-day streak")),
        (days, false) => parts.push(format!("🔥 {days}-day streak, play today to keep it")),
    }
    parts.join(" · ")
}

async fn fetch(server: &Url, token: &str) -> Result<Today, Failure> {
    crate::server::get_json(server, token, "/api/today", &[]).await
}

pub async fn run(app: AppHandle) {
    crate::server::poll(app, FIRST, EVERY, |app| async move { check(&app).await }).await;
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config();
    let today = match (
        config.server.as_deref().and_then(|s| Url::parse(s).ok()),
        config.token.clone(),
    ) {
        (Some(server), Some(token)) if !state.signed_out() => match fetch(&server, &token).await {
            Ok(today) => Some(today),
            Err(Failure::Offline) => return,
            Err(_) => None,
        },
        _ => None,
    };
    if let (Some(today), Some((hour, date))) = (today, local_now()) {
        let config = state.config();
        if remind(
            &today,
            hour,
            &date,
            config.streak_reminder,
            config.last_streak_reminder.as_deref(),
        ) {
            let (title, body) = reminder(&today);
            if crate::notify::show(app, &title, &body, crate::notify::Click::Page("/".into()))
                .await
                .is_ok()
            {
                state.update(|config| config.last_streak_reminder = Some(date));
            }
        }
    }
    if state.set_today(today) {
        crate::tray::refresh(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_up_the_day_in_one_line() {
        let busy = Today {
            plays: 42,
            listening_ms: 10_260_000,
            streak: 121,
            played_today: true,
        };
        assert_eq!(
            text(&busy),
            "Today: 42 plays · 2 h 51 min · 🔥 121-day streak"
        );
        let short = Today {
            plays: 1,
            listening_ms: 200_000,
            streak: 0,
            played_today: true,
        };
        assert_eq!(text(&short), "Today: 1 play · 3 min");
        let hours = Today {
            plays: 30,
            listening_ms: 7_200_000,
            streak: 5,
            played_today: true,
        };
        assert_eq!(text(&hours), "Today: 30 plays · 2 h · 🔥 5-day streak");
    }

    #[test]
    fn reminds_you_before_the_streak_breaks() {
        let quiet = Today {
            plays: 0,
            listening_ms: 0,
            streak: 121,
            played_today: false,
        };
        assert_eq!(
            text(&quiet),
            "Today: no plays yet · 🔥 121-day streak, play today to keep it"
        );
        let fresh = Today {
            plays: 0,
            listening_ms: 0,
            streak: 0,
            played_today: false,
        };
        assert_eq!(text(&fresh), "Today: no plays yet");
    }

    #[test]
    fn reminds_once_an_evening_when_the_streak_is_about_to_end() {
        let quiet = Today {
            plays: 0,
            listening_ms: 0,
            streak: 121,
            played_today: false,
        };
        let date = "2026-11-14";
        assert!(remind(&quiet, 21, date, true, None));
        assert!(remind(&quiet, 23, date, true, Some("2026-11-13")));
        assert!(!remind(&quiet, 20, date, true, None));
        assert!(!remind(&quiet, 22, date, true, Some(date)));
        assert!(!remind(&quiet, 22, date, false, None));
        assert!(!remind(
            &Today {
                played_today: true,
                plays: 3,
                ..quiet
            },
            22,
            date,
            true,
            None
        ));
        assert!(!remind(&Today { streak: 0, ..quiet }, 22, date, true, None));
        assert_eq!(reminder(&quiet).0, "Your 121-day streak ends at midnight");
    }

    #[test]
    fn reads_the_server_answer() {
        let today: Today = serde_json::from_str(
            r#"{"plays":2,"listeningMs":400000,"streak":2,"playedToday":true}"#,
        )
        .unwrap();
        assert_eq!(text(&today), "Today: 2 plays · 7 min · 🔥 2-day streak");
    }
}
