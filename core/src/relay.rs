use std::io::Read;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::time::timeout;
use url::Url;

const LONGEST_HEAD: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct Relay {
    inner: Arc<RelayInner>,
}

#[derive(Debug)]
struct RelayInner {
    port: u16,
    secret: String,
    stop: Arc<Notify>,
}

impl Drop for RelayInner {
    fn drop(&mut self) {
        self.stop.notify_waiters();
    }
}

fn new_secret() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| error.to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex_value(*bytes.get(i + 1)?)?;
            let lo = hex_value(*bytes.get(i + 2)?)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[derive(Debug, PartialEq, Eq)]
pub struct Asked {
    pub kind: String,
    pub id: String,
    pub range: Option<String>,
}

pub fn parse_head(head: &str, secret: &str) -> Option<Asked> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    if first.next()? != "GET" {
        return None;
    }
    let path = first.next()?;
    let mut parts = path.trim_start_matches('/').splitn(4, '/');
    if parts.next()? != "s" || parts.next()? != secret {
        return None;
    }
    let kind = parts.next()?;
    if kind != "full-song" && kind != "listen" {
        return None;
    }
    let raw_id = parts
        .next()
        .filter(|id| !id.is_empty() && !id.contains('/'))?;
    let id = percent_decode(raw_id)?;
    if id.is_empty() || id.contains('/') || id.contains('\\') || id == "." || id == ".." {
        return None;
    }
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("range")
            .then(|| value.trim().to_string())
    });
    Some(Asked {
        kind: kind.to_string(),
        id,
        range,
    })
}

pub fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let start: u64 = start.parse().ok()?;
    let end: u64 = end.parse().ok()?;
    let total: u64 = total.parse().ok()?;
    (start <= end).then_some((start, end, total))
}

pub fn response_head(status: u16, kind: &str, range: Option<&str>, length: Option<u64>) -> String {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        416 => "Range Not Satisfiable",
        404 => "Not Found",
        _ => "Bad Gateway",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n");
    let length = range
        .and_then(content_range)
        .and_then(|(start, end, _)| end.checked_sub(start)?.checked_add(1))
        .or(length);
    if let Some(range) = range {
        head.push_str(&format!("Content-Range: {range}\r\n"));
    }
    if let Some(length) = length {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    head.push_str("\r\n");
    head
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.take(LONGEST_HEAD as u64 + 1));
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let read = timeout(Duration::from_secs(10), reader.read_line(&mut line))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            return None;
        }
        head.push_str(&line);
        if head.len() > LONGEST_HEAD {
            return None;
        }
        if line == "\r\n" || line == "\n" {
            return Some(head.replace("\r\n", "\n").replace('\n', "\r\n"));
        }
    }
}

async fn fail(stream: &mut TcpStream, status: u16) {
    let _ = stream
        .write_all(response_head(status, "text/plain", None, Some(0)).as_bytes())
        .await;
}

async fn serve(mut stream: TcpStream, server: Url, token: String, secret: String) {
    let Some(asked) = read_head(&mut stream)
        .await
        .as_deref()
        .and_then(|head| parse_head(head, &secret))
    else {
        fail(&mut stream, 404).await;
        return;
    };
    let Ok(mut target) = server.join("/") else {
        fail(&mut stream, 502).await;
        return;
    };
    if let Ok(mut segments) = target.path_segments_mut() {
        segments
            .clear()
            .push("api")
            .push(&asked.kind)
            .push(&asked.id);
    }
    let whole = asked.range.is_none();
    let range = asked.range.unwrap_or_else(|| "bytes=0-".to_string());
    let upstream = crate::server::client()
        .get(target)
        .header("authorization", format!("Token {token}"))
        .header("range", range)
        .send()
        .await;
    let Ok(mut upstream) = upstream else {
        fail(&mut stream, 502).await;
        return;
    };
    let status = upstream.status().as_u16();
    if status != 200 && status != 206 {
        fail(&mut stream, if status == 416 { 416 } else { 404 }).await;
        return;
    }
    let header = |name: &str| {
        upstream
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let kind = header("content-type").unwrap_or_else(|| "application/octet-stream".into());
    let range = header("content-range").filter(|_| status == 206);
    let head = match range.as_deref().and_then(content_range) {
        Some((0, _, total)) if whole => response_head(200, &kind, None, Some(total)),
        _ => response_head(status, &kind, range.as_deref(), upstream.content_length()),
    };
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    while let Ok(Some(chunk)) = upstream.chunk().await {
        if stream.write_all(&chunk).await.is_err() {
            return;
        }
    }
}

impl Relay {
    pub fn start(server: Url, token: String) -> Result<Relay, String> {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let secret = new_secret()?;
        let expected = secret.clone();
        let stop = Arc::new(Notify::new());
        let stopped = stop.clone();
        let _guard = crate::host::async_runtime::runtime().enter();
        let listener = TcpListener::from_std(listener).map_err(|error| error.to_string())?;
        crate::host::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = stopped.notified() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let server = server.clone();
                        let token = token.clone();
                        let secret = expected.clone();
                        tokio::spawn(async move {
                            serve(stream, server, token, secret).await;
                        });
                    }
                }
            }
        });
        Ok(Relay {
            inner: Arc::new(RelayInner { port, secret, stop }),
        })
    }

    pub fn address(&self, kind: &str, id: &str) -> Option<Url> {
        let mut url = Url::parse(&format!("http://127.0.0.1:{}/", self.inner.port)).ok()?;
        url.path_segments_mut()
            .ok()?
            .clear()
            .push("s")
            .push(&self.inner.secret)
            .push(kind)
            .push(id);
        Some(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_player_asks_for() {
        let head =
            "GET /s/k3y/full-song/nd-1 HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=100-\r\n\r\n";
        assert_eq!(
            parse_head(head, "k3y"),
            Some(Asked {
                kind: "full-song".into(),
                id: "nd-1".into(),
                range: Some("bytes=100-".into()),
            })
        );
        assert_eq!(
            parse_head("GET /s/k3y/listen/band-song HTTP/1.1\r\n\r\n", "k3y")
                .map(|asked| asked.range),
            Some(None)
        );
        assert_eq!(
            parse_head("GET /s/full-song/nd-1 HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/wrong/full-song/nd-1 HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/k3y/admin/x HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("POST /s/k3y/full-song/x HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/k3y/full-song/a/b HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(parse_head("GET /other HTTP/1.1\r\n\r\n", "k3y"), None);
        assert_eq!(new_secret().unwrap().len(), 32);
        assert_ne!(new_secret().unwrap(), new_secret().unwrap());
    }

    #[test]
    fn decodes_song_ids_once_and_rejects_separators() {
        let server = Url::parse("https://music.example/").unwrap();
        let relay = Relay::start(server.clone(), "t".into()).unwrap();
        let address = relay.address("listen", "band-a b%20c#1?x").unwrap();
        let head = format!("GET {} HTTP/1.1\r\n\r\n", address.path());
        let asked = parse_head(&head, &relay.inner.secret).unwrap();
        assert_eq!(asked.id, "band-a b%20c#1?x");
        let mut target = server.join("/").unwrap();
        target
            .path_segments_mut()
            .unwrap()
            .clear()
            .push("api")
            .push(&asked.kind)
            .push(&asked.id);
        assert_eq!(
            target.as_str(),
            "https://music.example/api/listen/band-a%20b%2520c%231%3Fx"
        );
        assert_eq!(
            parse_head("GET /s/k3y/full-song/a%2Fb HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/k3y/full-song/a%5Cb HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/k3y/full-song/%2e%2e HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
        assert_eq!(
            parse_head("GET /s/k3y/full-song/%ZZ HTTP/1.1\r\n\r\n", "k3y"),
            None
        );
    }

    #[test]
    fn tells_the_player_how_long_the_song_is() {
        assert_eq!(
            content_range("bytes 0-482732/482733"),
            Some((0, 482_732, 482_733))
        );
        assert_eq!(content_range("bytes */100"), None);
        assert_eq!(content_range("bytes 200-100/1000"), None);
        let head = response_head(206, "audio/mpeg", Some("bytes 100-199/1000"), None);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"));
        assert!(head.contains("Content-Length: 100\r\n"));
        assert!(head.contains("Content-Range: bytes 100-199/1000\r\n"));
        assert!(head.contains("Accept-Ranges: bytes\r\n"));
        let bad = response_head(206, "audio/mpeg", Some("bytes 200-100/1000"), None);
        assert!(!bad.contains("Content-Length"));
        let plain = response_head(200, "audio/mpeg", None, Some(5000));
        assert!(plain.contains("Content-Length: 5000\r\n"));
        assert!(!plain.contains("Content-Range"));
        assert!(!response_head(200, "audio/mpeg", None, None).contains("Content-Length"));
    }
}
