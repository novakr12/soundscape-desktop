use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

const EVERY: Duration = Duration::from_secs(3 * 60 * 60);
const FIRST: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Artist {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Place {
    pub rank: u32,
    pub of: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Recap {
    pub from: String,
    pub plays: u64,
    pub listening_ms: u64,
    pub top_artist: Option<Artist>,
    pub place: Option<Place>,
}

#[derive(Debug, Deserialize)]
struct Answer {
    recap: Option<Recap>,
    show: bool,
}

fn duration(ms: u64) -> String {
    let minutes = ms / 60_000;
    if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{} h {} min", minutes / 60, minutes % 60)
    }
}

pub fn body(recap: &Recap) -> String {
    let mut parts = vec![
        if recap.plays == 1 {
            "1 play".to_string()
        } else {
            format!("{} plays", recap.plays)
        },
        duration(recap.listening_ms),
    ];
    if let Some(artist) = &recap.top_artist {
        parts.push(format!("most played: {}", artist.name));
    }
    if let Some(place) = recap.place.as_ref().filter(|place| place.of > 1) {
        parts.push(format!("#{} of {} among friends", place.rank, place.of));
    }
    parts.join(" · ")
}

pub fn due(show: bool, recap: Option<&Recap>, last: Option<&str>) -> bool {
    show && recap.is_some_and(|recap| last != Some(recap.from.as_str()))
}

async fn fetch(server: &Url, token: &str) -> Result<(bool, Option<Recap>), Failure> {
    crate::server::get_json::<Answer>(server, token, "/api/recap/week", &[])
        .await
        .map(|answer| (answer.show, answer.recap))
}

pub async fn run(app: AppHandle) {
    crate::server::poll(app, FIRST, EVERY, |app| async move { check(&app).await }).await;
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config();
    let Some((server, token)) = crate::server::credentials(app) else {
        return;
    };
    if !config.weekly_recap {
        return;
    }
    let Ok((show, recap)) = fetch(&server, &token).await else {
        return;
    };
    if !due(show, recap.as_ref(), config.last_recap.as_deref()) {
        return;
    }
    let Some(recap) = recap else { return };
    if crate::notify::show_notification(app, "Your week in music", &body(&recap)).is_ok() {
        state.update(|config| config.last_recap = Some(recap.from.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recap() -> Recap {
        Recap {
            from: "2026-11-09".into(),
            plays: 142,
            listening_ms: 31_500_000,
            top_artist: Some(Artist {
                name: "Björk".into(),
            }),
            place: Some(Place { rank: 2, of: 5 }),
        }
    }

    #[test]
    fn sums_up_the_week_in_one_line() {
        assert_eq!(
            body(&recap()),
            "142 plays · 8 h 45 min · most played: Björk · #2 of 5 among friends"
        );
        let quiet = Recap {
            plays: 1,
            listening_ms: 200_000,
            top_artist: None,
            place: Some(Place { rank: 1, of: 1 }),
            ..recap()
        };
        assert_eq!(body(&quiet), "1 play · 3 min");
    }

    #[test]
    fn shows_each_week_once_and_only_early_in_the_week() {
        let week = recap();
        assert!(due(true, Some(&week), None));
        assert!(due(true, Some(&week), Some("2026-11-02")));
        assert!(!due(true, Some(&week), Some("2026-11-09")));
        assert!(!due(false, Some(&week), None));
        assert!(!due(true, None, None));
    }

    #[test]
    fn reads_the_server_answer() {
        let answer: Answer = serde_json::from_str(r#"{"recap":{"from":"2026-11-09","to":"2026-11-15","plays":3,"playsBefore":0,"listeningMs":600000,"newArtists":1,"topArtist":{"artistId":"a","name":"Air","plays":3}},"show":true,"url":"/"}"#).unwrap();
        assert!(answer.show);
        assert_eq!(
            body(&answer.recap.unwrap()),
            "3 plays · 10 min · most played: Air"
        );
    }
}
