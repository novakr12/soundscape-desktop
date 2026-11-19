use crate::host::{AppHandle, Manager};
use crate::submit::Failure;
use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use url::Url;

const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const FIRST: Duration = Duration::from_secs(45);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Ready {
    pub ready: bool,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub plays: u64,
    #[serde(default)]
    pub listening_ms: u64,
    #[serde(default)]
    pub top_artist: Option<String>,
    #[serde(default)]
    pub new_artists: u64,
}

fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

pub fn due(answer: &Ready, enabled: bool, last: Option<i32>) -> Option<i32> {
    let year = answer.year?;
    (enabled && answer.ready && answer.plays > 0 && last != Some(year)).then_some(year)
}

pub fn notification(answer: &Ready) -> (String, String) {
    let year = answer.year.unwrap_or_default();
    let mut parts = vec![format!("{} plays", thousands(answer.plays))];
    let hours = (answer.listening_ms + 1_800_000) / 3_600_000;
    if hours > 0 {
        parts.push(format!("{} h", thousands(hours)));
    }
    if let Some(artist) = &answer.top_artist {
        parts.push(format!("most played: {artist}"));
    }
    match answer.new_artists {
        0 => {}
        1 => parts.push("1 new artist".into()),
        count => parts.push(format!("{} new artists", thousands(count))),
    }
    (
        format!("Your {year} Yearscape is ready"),
        format!(
            "{}. Open Soundscape to see the whole year.",
            parts.join(" · ")
        ),
    )
}

async fn fetch(server: &Url, token: &str) -> Result<Ready, Failure> {
    crate::server::get_json(server, token, "/api/yearscape", &[]).await
}

pub async fn run(app: AppHandle) {
    crate::server::poll(app, FIRST, EVERY, |app| async move { check(&app).await }).await;
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config();
    let (Some(server), Some(token)) = (
        config.server.as_deref().and_then(|s| Url::parse(s).ok()),
        config.token.clone(),
    ) else {
        return;
    };
    if !config.yearscape_notification || state.signed_out() {
        return;
    }
    let Ok(answer) = fetch(&server, &token).await else {
        return;
    };
    let Some(year) = due(
        &answer,
        config.yearscape_notification,
        config.last_yearscape,
    ) else {
        return;
    };
    let (title, body) = notification(&answer);
    if crate::notify::show_notification(app, &title, &body).is_ok() {
        state.update(|config| config.last_yearscape = Some(year));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> Ready {
        serde_json::from_str(r#"{"ready":true,"year":2026,"plays":3412,"listeningMs":770400000,"topArtist":"Radiohead","topSong":{"title":"Reckoner","artist":"Radiohead"},"newArtists":31,"url":"/yearscape?year=2026"}"#).unwrap()
    }

    #[test]
    fn teases_the_year_in_one_line() {
        assert_eq!(
            notification(&answer()),
            (
                "Your 2026 Yearscape is ready".into(),
                "3,412 plays · 214 h · most played: Radiohead · 31 new artists. Open Soundscape to see the whole year.".into()
            )
        );
        let small = Ready {
            plays: 3,
            listening_ms: 600_000,
            top_artist: None,
            new_artists: 1,
            ..answer()
        };
        assert_eq!(
            notification(&small).1,
            "3 plays · 1 new artist. Open Soundscape to see the whole year."
        );
    }

    #[test]
    fn tells_you_once_a_year() {
        assert_eq!(due(&answer(), true, None), Some(2026));
        assert_eq!(due(&answer(), true, Some(2025)), Some(2026));
        assert_eq!(due(&answer(), true, Some(2026)), None);
        assert_eq!(due(&answer(), false, None), None);
        let not_yet: Ready = serde_json::from_str(r#"{"ready":false}"#).unwrap();
        assert_eq!(due(&not_yet, true, None), None);
    }
}
