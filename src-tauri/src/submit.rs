use crate::scrobble::Track;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

pub const BATCH: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listen {
    pub listened_at: i64,
    pub track: Track,
    pub player: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    Offline,
    SignedOut,
    Refused(u16),
}

fn metadata(track: &Track, player: &str) -> Value {
    let mut info = json!({
        "media_player": player,
        "submission_client": "Soundscape Desktop",
        "submission_client_version": env!("CARGO_PKG_VERSION"),
    });
    if let Some(length) = track.length_ms {
        info["duration_ms"] = json!(length);
    }
    let mut metadata = json!({
        "artist_name": track.artist,
        "track_name": track.title,
        "additional_info": info,
    });
    if let Some(album) = &track.album {
        metadata["release_name"] = json!(album);
    }
    metadata
}

pub fn now_playing_body(track: &Track, player: &str) -> Value {
    json!({ "listen_type": "playing_now", "payload": [{ "track_metadata": metadata(track, player) }] })
}

pub fn listens_body(listens: &[Listen]) -> Value {
    let payload: Vec<Value> = listens
        .iter()
        .map(|listen| json!({ "listened_at": listen.listened_at, "track_metadata": metadata(&listen.track, &listen.player) }))
        .collect();
    json!({ "listen_type": if listens.len() == 1 { "single" } else { "import" }, "payload": payload })
}

async fn post(server: &Url, token: &str, body: &Value) -> Result<(), Failure> {
    let address = server
        .join("/1/submit-listens")
        .map_err(|_| Failure::Refused(0))?;
    let response = crate::server::client()
        .post(address)
        .header("authorization", format!("Token {token}"))
        .json(body)
        .send()
        .await
        .map_err(|_| Failure::Offline)?;
    let status = response.status().as_u16();
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(Failure::SignedOut),
        408 | 429 | 500..=599 => Err(Failure::Offline),
        _ => Err(Failure::Refused(status)),
    }
}

pub async fn now_playing(
    server: &Url,
    token: &str,
    track: &Track,
    player: &str,
) -> Result<(), Failure> {
    post(server, token, &now_playing_body(track, player)).await
}

pub async fn listens(server: &Url, token: &str, listens: &[Listen]) -> Result<(), Failure> {
    post(server, token, &listens_body(listens)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn track() -> Track {
        Track {
            artist: "Radiohead".into(),
            title: "Airbag".into(),
            album: Some("OK Computer".into()),
            length_ms: Some(284_000),
        }
    }

    #[test]
    fn sends_what_listenbrainz_servers_expect() {
        let body = now_playing_body(&track(), "Strawberry");
        assert_eq!(body["listen_type"], "playing_now");
        assert_eq!(
            body["payload"][0]["track_metadata"]["artist_name"],
            "Radiohead"
        );
        assert_eq!(
            body["payload"][0]["track_metadata"]["release_name"],
            "OK Computer"
        );
        assert_eq!(
            body["payload"][0]["track_metadata"]["additional_info"]["duration_ms"],
            284_000
        );
        assert_eq!(
            body["payload"][0]["track_metadata"]["additional_info"]["media_player"],
            "Strawberry"
        );
        assert!(body["payload"][0].get("listened_at").is_none());

        let one = Listen {
            listened_at: 1_700_000_000,
            track: track(),
            player: "Strawberry".into(),
        };
        assert_eq!(
            listens_body(std::slice::from_ref(&one))["listen_type"],
            "single"
        );
        let many = listens_body(&[one.clone(), one]);
        assert_eq!(many["listen_type"], "import");
        assert_eq!(many["payload"][1]["listened_at"], 1_700_000_000);

        let bare = Track {
            album: None,
            length_ms: None,
            ..track()
        };
        let body = now_playing_body(&bare, "x");
        assert!(body["payload"][0]["track_metadata"]
            .get("release_name")
            .is_none());
        assert!(body["payload"][0]["track_metadata"]["additional_info"]
            .get("duration_ms")
            .is_none());
    }

    fn answer(status: &'static str) -> (Url, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while let Ok(read) = stream.read(&mut buffer) {
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length || read == 0 {
                            break;
                        }
                    }
                }
                let _ = send.send(String::from_utf8_lossy(&request).to_string());
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
                    )
                    .as_bytes(),
                );
            }
        });
        (
            Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap(),
            receive,
        )
    }

    #[test]
    fn tells_apart_offline_signed_out_and_refused() {
        let run = |future| tauri::async_runtime::block_on(future);
        let song = track();
        let (ok, request) = answer("200 OK");
        assert_eq!(run(now_playing(&ok, "secret", &song, "Strawberry")), Ok(()));
        let request = request.recv().unwrap();
        assert!(request.starts_with("POST /1/submit-listens"));
        assert!(request
            .to_lowercase()
            .contains("authorization: token secret"));
        assert!(request.contains("\"playing_now\""));

        let (signed_out, _) = answer("401 Unauthorized");
        assert_eq!(
            run(now_playing(&signed_out, "old", &song, "x")),
            Err(Failure::SignedOut)
        );
        let (busy, _) = answer("503 Service Unavailable");
        assert_eq!(
            run(now_playing(&busy, "t", &song, "x")),
            Err(Failure::Offline)
        );
        let (bad, _) = answer("400 Bad Request");
        assert_eq!(
            run(now_playing(&bad, "t", &song, "x")),
            Err(Failure::Refused(400))
        );
        let closed = Url::parse("http://127.0.0.1:9/").unwrap();
        assert_eq!(
            run(now_playing(&closed, "t", &song, "x")),
            Err(Failure::Offline)
        );
    }
}
