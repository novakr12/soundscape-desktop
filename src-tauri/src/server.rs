use crate::submit::Failure;
use crate::AppState;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::future::Future;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use url::Url;

pub const APP_SCHEME: &str = "soundscape-desktop";
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct Info {
    app: String,
    api: u32,
}

pub fn normalize_address(input: &str) -> Result<Url, String> {
    let text = input.trim();
    if text.is_empty() {
        return Err("Enter the address you open Soundscape at.".into());
    }
    let with_scheme = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let url = Url::parse(&with_scheme)
        .map_err(|_| "That doesn't look like a web address.".to_string())?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err("The address has to start with https:// or http://.".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("That doesn't look like a web address.".into());
    }
    Url::parse(&url.origin().ascii_serialization())
        .map_err(|_| "That doesn't look like a web address.".into())
}

pub fn client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(TIMEOUT)
                .user_agent(concat!("SoundscapeDesktop/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("HTTP client")
        })
        .clone()
}

pub fn failure_for(status: u16) -> Failure {
    match status {
        401 | 403 => Failure::SignedOut,
        408 | 429 | 500..=599 => Failure::Offline,
        _ => Failure::Refused(status),
    }
}

pub fn credentials(app: &AppHandle) -> Option<(Url, String)> {
    let state = app.state::<AppState>();
    if state.signed_out() {
        return None;
    }
    let config = state.config();
    Some((Url::parse(config.server.as_deref()?).ok()?, config.token?))
}

pub fn address(server: &Url, path: &str) -> Result<Url, Failure> {
    server.join(path).map_err(|_| Failure::Refused(0))
}

pub async fn answer(
    request: reqwest::RequestBuilder,
    token: &str,
) -> Result<reqwest::Response, Failure> {
    let response = request
        .header("authorization", format!("Token {token}"))
        .send()
        .await
        .map_err(|_| Failure::Offline)?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        Ok(response)
    } else {
        Err(failure_for(status))
    }
}

pub async fn get_json<T: DeserializeOwned>(
    server: &Url,
    token: &str,
    path: &str,
    query: &[(&str, &str)],
) -> Result<T, Failure> {
    let mut target = address(server, path)?;
    if !query.is_empty() {
        target.query_pairs_mut().extend_pairs(query);
    }
    answer(client().get(target), token)
        .await?
        .json::<T>()
        .await
        .map_err(|_| Failure::Refused(200))
}

pub async fn poll<F, Fut>(app: AppHandle, first: Duration, every: Duration, check: F)
where
    F: Fn(AppHandle) -> Fut,
    Fut: Future<Output = ()>,
{
    tokio::time::sleep(first).await;
    loop {
        check(app.clone()).await;
        tokio::time::sleep(every).await;
    }
}

pub async fn check_server(server: &Url) -> Result<(), String> {
    let address = server
        .join("/api/info")
        .map_err(|error| error.to_string())?;
    let response = client().get(address).send().await.map_err(|_| {
        format!(
            "Couldn't reach {}. Check the address and that the server is running.",
            server.as_str().trim_end_matches('/')
        )
    })?;
    if !response.status().is_success() {
        return Err("This server answered, but it isn't Soundscape, or it needs an update.".into());
    }
    match response.json::<Info>().await {
        Ok(info) if info.app == "soundscape" && info.api >= 1 => Ok(()),
        _ => Err("This server answered, but it isn't Soundscape, or it needs an update.".into()),
    }
}

pub async fn revoke_token(server: &Url, token: &str) {
    if let Ok(address) = server.join("/api/token") {
        let _ = client()
            .delete(address)
            .header("authorization", format!("Token {token}"))
            .send()
            .await;
    }
}

pub fn connect_url(server: &Url, device: &str) -> Url {
    let mut url = server.join("/desktop/connect").expect("connect address");
    url.query_pairs_mut().append_pair("device", device);
    url
}

pub fn device_name() -> String {
    let host = gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_string();
    if host.is_empty() || host == "localhost" {
        "Desktop app".into()
    } else {
        format!("Desktop app on {host}")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Navigation {
    Allow,
    AppSettings,
    Connected { token: String, user: String },
    External,
    Block,
}

pub const CONNECTED_PATH: &str = "/desktop/connected";

fn connected(pairs: url::form_urlencoded::Parse<'_>) -> Option<Navigation> {
    let pairs: std::collections::HashMap<_, _> = pairs.into_owned().collect();
    match (pairs.get("token"), pairs.get("user")) {
        (Some(token), Some(user)) if !token.is_empty() && !user.is_empty() => {
            Some(Navigation::Connected {
                token: token.clone(),
                user: user.clone(),
            })
        }
        _ => None,
    }
}

pub fn classify(server: Option<&Url>, target: &Url) -> Navigation {
    if target.scheme() == APP_SCHEME {
        if target.host_str() == Some("settings") {
            return Navigation::AppSettings;
        }
        if target.host_str() != Some("connected") {
            return Navigation::Block;
        }
        return connected(target.query_pairs()).unwrap_or(Navigation::Block);
    }
    if server.is_some_and(|server| server.origin() == target.origin())
        && target.path() == CONNECTED_PATH
    {
        if let Some(found) = target
            .fragment()
            .and_then(|fragment| connected(url::form_urlencoded::parse(fragment.as_bytes())))
        {
            return found;
        }
    }
    match target.scheme() {
        "tauri" | "about" | "data" | "blob" => Navigation::Allow,
        "http" | "https" => {
            let local = target.host_str() == Some("tauri.localhost");
            let own = server.is_some_and(|server| server.origin() == target.origin());
            if local || own {
                Navigation::Allow
            } else {
                Navigation::External
            }
        }
        "mailto" => Navigation::External,
        _ => Navigation::Block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn cleans_up_typed_addresses() {
        assert_eq!(
            normalize_address("music.example.com").unwrap().as_str(),
            "https://music.example.com/"
        );
        assert_eq!(
            normalize_address(" http://192.168.1.20:3000/history ")
                .unwrap()
                .as_str(),
            "http://192.168.1.20:3000/"
        );
        assert_eq!(
            normalize_address("https://box.tail1234.ts.net/1/")
                .unwrap()
                .as_str(),
            "https://box.tail1234.ts.net/"
        );
        assert!(normalize_address("").is_err());
        assert!(normalize_address("ftp://example.com").is_err());
        assert!(normalize_address("https://").is_err());
    }

    #[test]
    fn builds_the_connect_address_with_the_device_name() {
        let address = connect_url(&url("https://music.example.com/"), "Desktop app on laptop");
        assert_eq!(
            address.as_str(),
            "https://music.example.com/desktop/connect?device=Desktop+app+on+laptop"
        );
    }

    #[test]
    fn decides_where_links_open() {
        let server = url("https://music.example.com/");
        let here = Some(&server);
        assert_eq!(
            classify(here, &url("https://music.example.com/stats")),
            Navigation::Allow
        );
        assert_eq!(
            classify(here, &url("tauri://localhost/index.html")),
            Navigation::Allow
        );
        assert_eq!(
            classify(here, &url("http://tauri.localhost/index.html")),
            Navigation::Allow
        );
        assert_eq!(
            classify(here, &url("https://musicbrainz.org/artist/x")),
            Navigation::External
        );
        assert_eq!(
            classify(here, &url("http://music.example.com/stats")),
            Navigation::External
        );
        assert_eq!(
            classify(None, &url("https://music.example.com/")),
            Navigation::External
        );
        assert_eq!(
            classify(here, &url("file:///etc/passwd")),
            Navigation::Block
        );
        assert_eq!(
            classify(
                here,
                &url("soundscape-desktop://connected?token=abc&user=listener")
            ),
            Navigation::Connected {
                token: "abc".into(),
                user: "listener".into()
            }
        );
        assert_eq!(
            classify(here, &url("soundscape-desktop://connected?token=&user=x")),
            Navigation::Block
        );
        assert_eq!(
            classify(here, &url("soundscape-desktop://other?token=a&user=b")),
            Navigation::Block
        );
        assert_eq!(
            classify(here, &url("soundscape-desktop://settings")),
            Navigation::AppSettings
        );
        assert_eq!(
            classify(
                here,
                &url("https://music.example.com/desktop/connected#token=abc&user=listener")
            ),
            Navigation::Connected {
                token: "abc".into(),
                user: "listener".into()
            }
        );
        assert_eq!(
            classify(here, &url("https://music.example.com/desktop/connected")),
            Navigation::Allow
        );
        assert_eq!(
            classify(
                here,
                &url("https://music.example.com/desktop/connected#token=&user=x")
            ),
            Navigation::Allow
        );
        assert_eq!(
            classify(
                here,
                &url("https://evil.example.org/desktop/connected#token=abc&user=listener")
            ),
            Navigation::External
        );
        assert_eq!(
            classify(
                None,
                &url("https://music.example.com/desktop/connected#token=abc&user=x")
            ),
            Navigation::External
        );
    }

    fn serve_once(body: &'static str, status: &'static str) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 2048];
                let _ = stream.read(&mut buffer);
                let reply = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        url(&format!("http://127.0.0.1:{port}/"))
    }

    #[test]
    fn sorts_answers_into_signed_out_try_later_and_refused() {
        assert!(matches!(failure_for(401), Failure::SignedOut));
        assert!(matches!(failure_for(403), Failure::SignedOut));
        assert!(matches!(failure_for(429), Failure::Offline));
        assert!(matches!(failure_for(503), Failure::Offline));
        assert!(matches!(failure_for(404), Failure::Refused(404)));
    }

    #[derive(Debug, Deserialize)]
    struct Echo {
        ok: bool,
    }

    #[test]
    fn reads_json_from_the_server_with_the_token() {
        let good = serve_once(r#"{"ok":true}"#, "200 OK");
        let answer: Echo =
            tauri::async_runtime::block_on(get_json(&good, "t", "/api/x", &[("q", "a b")]))
                .unwrap();
        assert!(answer.ok);
        let gone = serve_once("{}", "401 Unauthorized");
        assert!(matches!(
            tauri::async_runtime::block_on(get_json::<Echo>(&gone, "t", "/api/x", &[])),
            Err(Failure::SignedOut)
        ));
        let busy = serve_once("{}", "503 Service Unavailable");
        assert!(matches!(
            tauri::async_runtime::block_on(get_json::<Echo>(&busy, "t", "/api/x", &[])),
            Err(Failure::Offline)
        ));
        let odd = serve_once("not json", "200 OK");
        assert!(matches!(
            tauri::async_runtime::block_on(get_json::<Echo>(&odd, "t", "/api/x", &[])),
            Err(Failure::Refused(200))
        ));
    }

    #[test]
    fn recognises_a_soundscape_server() {
        let good = serve_once(
            r#"{"app":"soundscape","version":"0.1.0","api":1}"#,
            "200 OK",
        );
        assert_eq!(tauri::async_runtime::block_on(check_server(&good)), Ok(()));
        let other = serve_once(r#"{"name":"something else"}"#, "200 OK");
        assert!(tauri::async_runtime::block_on(check_server(&other))
            .unwrap_err()
            .contains("isn't Soundscape"));
        let missing = serve_once("{}", "404 Not Found");
        assert!(tauri::async_runtime::block_on(check_server(&missing))
            .unwrap_err()
            .contains("isn't Soundscape"));
        let closed = url("http://127.0.0.1:9/");
        assert!(tauri::async_runtime::block_on(check_server(&closed))
            .unwrap_err()
            .starts_with("Couldn't reach"));
    }
}
