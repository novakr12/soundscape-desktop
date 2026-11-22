use crate::host::{async_runtime, AppHandle, Manager};
use crate::{server, AppState, MAIN};
use url::Url;

pub use soundscape_protocol::setup::{asked, page};
pub use soundscape_protocol::SETUP_HOST;

pub async fn connect(app: &AppHandle, address: &str) -> Result<Url, String> {
    let server = server::normalize_address(address)?;
    server::check_server(&server).await?;
    let state = app.state::<AppState>();
    let previous = state.config();
    if let (Some(old), Some(token)) = (
        previous.server.as_deref().and_then(|s| Url::parse(s).ok()),
        previous.token.as_deref(),
    ) {
        if old != server {
            server::revoke_token(&old, token).await;
        }
    }
    let same = previous.server.as_deref() == Some(server.as_str())
        && previous.connected()
        && !state.signed_out();
    if !same {
        state.update(|config| {
            config.server = Some(server.to_string());
            config.user = None;
            config.token = None;
            config.last_notification = None;
        });
    }
    Ok(if same {
        server.clone()
    } else {
        server::connect_url(&server, &server::device_name())
    })
}

pub fn follow(app: &AppHandle, target: &Url) -> bool {
    let Some(address) = asked(target) else {
        return false;
    };
    let app = app.clone();
    async_runtime::spawn(async move {
        let next = match connect(&app, &address).await {
            Ok(start) => start,
            Err(problem) => page(&address, Some(&problem)),
        };
        app.host().navigate(MAIN, next);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_for_the_server_and_reads_the_answer() {
        let shown = page("music.example.com", Some("Couldn't reach <it>"));
        assert_eq!(shown.scheme(), "data");
        let text = shown.as_str();
        assert!(text.contains("setup.soundscape-desktop.invalid"));
        assert!(!text.contains("<it>"));
        assert_eq!(
            asked(
                &Url::parse("http://setup.soundscape-desktop.invalid/?server=music.example.com")
                    .unwrap()
            ),
            Some("music.example.com".into())
        );
        assert_eq!(
            asked(&Url::parse("http://setup.soundscape-desktop.invalid/").unwrap()),
            Some(String::new())
        );
        assert_eq!(
            asked(&Url::parse("soundscape-desktop://setup?server=x").unwrap()),
            None
        );
        assert_eq!(
            asked(&Url::parse("https://music.example.com/setup").unwrap()),
            None
        );
    }
}
