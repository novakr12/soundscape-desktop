use crate::host::{async_runtime, AppHandle, Manager};
use crate::{server, AppState, MAIN};
use url::Url;

pub const SETUP_HOST: &str = "setup.soundscape-desktop.invalid";

const STYLE: &str = "body{margin:0;min-height:100vh;display:grid;place-items:center;font:16px system-ui,sans-serif;color-scheme:light dark;background:Canvas;color:CanvasText}main{width:min(420px,calc(100vw - 48px))}h1{font-size:1.5rem;margin:0 0 .5rem}p{line-height:1.5}.muted{opacity:.7}label{display:block;font-weight:600;margin:1.25rem 0 .4rem}input{box-sizing:border-box;width:100%;font:inherit;padding:.6rem .75rem;border:1px solid color-mix(in srgb,CanvasText 30%,transparent);border-radius:8px;background:Canvas;color:CanvasText}button{margin-top:1rem;font:inherit;font-weight:600;padding:.6rem 1.2rem;border:0;border-radius:8px;background:#1db954;color:#fff;cursor:pointer}.error{color:#d93025;font-weight:600}";

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn page(address: &str, problem: Option<&str>) -> Url {
    let problem = problem
        .map(|text| format!("<p class=\"error\" role=\"alert\">{}</p>", escape(text)))
        .unwrap_or_default();
    let html = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Connect to Soundscape</title><style>{STYLE}</style><main><h1>Connect to Soundscape</h1><p class=\"muted\">Enter the address you open Soundscape at in your browser.</p><form action=\"http://{SETUP_HOST}/\" method=\"get\"><label for=\"server\">Server address</label><input id=\"server\" name=\"server\" type=\"text\" inputmode=\"url\" autocomplete=\"url\" spellcheck=\"false\" placeholder=\"https://music.example.com\" value=\"{value}\" required autofocus><button type=\"submit\">Connect</button>{problem}</form><p class=\"muted\">You'll log in on the next page. The app gets its own token, which you can revoke on Soundscape's Connect page.</p></main></html>",
        value = escape(address),
    );
    let encoded: String = url::form_urlencoded::byte_serialize(html.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    Url::parse(&format!("data:text/html;charset=utf-8,{encoded}")).expect("setup page")
}

pub fn asked(target: &Url) -> Option<String> {
    let ours = matches!(target.scheme(), "http" | "https") && target.host_str() == Some(SETUP_HOST);
    if !ours {
        return None;
    }
    Some(
        target
            .query_pairs()
            .find(|(name, _)| name == "server")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default(),
    )
}

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
