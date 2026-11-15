use crate::AppState;
use serde::Deserialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const FIRST: Duration = Duration::from_secs(2 * 60);
const LATEST: &str = "https://api.github.com/repos/novakr12/soundscape-desktop/releases/latest";
pub const DOWNLOADS: &str = "https://github.com/novakr12/soundscape-desktop/releases/latest";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
}

fn parts(version: &str) -> Option<Vec<u64>> {
    let plain = version.trim().trim_start_matches(['v', 'V']);
    let plain = plain.split(['-', '+']).next()?;
    plain
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

pub fn version_of(release: &Release) -> Option<String> {
    if release.draft || release.prerelease {
        return None;
    }
    parts(&release.tag_name)?;
    Some(
        release
            .tag_name
            .trim()
            .trim_start_matches(['v', 'V'])
            .to_string(),
    )
}

pub fn newer(current: &str, latest: &str) -> bool {
    match (parts(current), parts(latest)) {
        (Some(mut current), Some(mut latest)) => {
            let length = current.len().max(latest.len());
            current.resize(length, 0);
            latest.resize(length, 0);
            latest > current
        }
        _ => false,
    }
}

pub fn should_tell(available: &str, enabled: bool, told: Option<&str>) -> bool {
    enabled && told != Some(available)
}

pub fn menu_text(version: &str) -> String {
    format!("Download Soundscape Desktop {version}…")
}

async fn latest() -> Option<Release> {
    crate::server::client()
        .get(LATEST)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Release>()
        .await
        .ok()
}

pub async fn run(app: AppHandle) {
    crate::server::poll(app, FIRST, EVERY, |app| async move { check(&app).await }).await;
}

pub async fn check(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.config().update_notice {
        return;
    }
    let Some(version) = latest().await.as_ref().and_then(version_of) else {
        return;
    };
    if !newer(CURRENT, &version) {
        return;
    }
    if state.set_update(Some(version.clone())) {
        crate::tray::refresh(app);
    }
    let config = state.config();
    if !should_tell(
        &version,
        config.update_notice,
        config.told_update.as_deref(),
    ) {
        return;
    }
    let body = format!(
        "You have {CURRENT}. Choose “{}” in the tray menu to get it.",
        menu_text(&version)
    );
    let title = format!("Soundscape Desktop {version} is out");
    if crate::notify::show_notification(app, &title, &body).is_ok() {
        state.update(|config| config.told_update = Some(version));
    }
}

pub fn open_downloads(app: &AppHandle) {
    if let Ok(page) = Url::parse(DOWNLOADS) {
        crate::links::open(app, &page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(json: &str) -> Release {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn compares_versions_number_by_number() {
        assert!(newer("0.6.3", "0.7.0"));
        assert!(newer("0.6.3", "v0.6.10"));
        assert!(newer("0.9.9", "1.0"));
        assert!(!newer("0.6.3", "0.6.3"));
        assert!(!newer("0.6.3", "v0.6.3"));
        assert!(!newer("0.7.0", "0.6.9"));
        assert!(!newer("1.0.0", "1"));
        assert!(!newer("0.6.3", "nightly"));
    }

    #[test]
    fn reads_the_version_from_the_release() {
        let json = r#"{"tag_name":"v0.7.0","name":"0.7.0","html_url":"https://example.com","draft":false,"prerelease":false}"#;
        assert_eq!(version_of(&release(json)).as_deref(), Some("0.7.0"));
        assert_eq!(
            version_of(&release(r#"{"tag_name":"0.8.1"}"#)).as_deref(),
            Some("0.8.1")
        );
        assert_eq!(
            version_of(&release(r#"{"tag_name":"v0.8.0","prerelease":true}"#)),
            None
        );
        assert_eq!(
            version_of(&release(r#"{"tag_name":"v0.8.0","draft":true}"#)),
            None
        );
        assert_eq!(version_of(&release(r#"{"tag_name":"latest"}"#)), None);
    }

    #[test]
    fn tells_about_each_version_once() {
        assert!(should_tell("0.7.0", true, None));
        assert!(should_tell("0.7.1", true, Some("0.7.0")));
        assert!(!should_tell("0.7.0", true, Some("0.7.0")));
        assert!(!should_tell("0.7.0", false, None));
    }

    #[test]
    fn names_the_version_in_the_menu() {
        assert_eq!(menu_text("0.7.0"), "Download Soundscape Desktop 0.7.0…");
    }
}
